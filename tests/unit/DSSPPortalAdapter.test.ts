import { afterEach, describe, expect, it, vi } from "vitest";

import {
  buildTrainingFormPayload,
  DSSPPortalAdapter,
  fillTrainingFormFields,
  getFormOptions,
  getTrainees,
  isSupportedPortalPage,
  isSupportedTraineePage,
  isAuthenticatedTraineePage,
  selectFormOption,
  waitForAuthenticatedTraineePage,
  waitForTraineeTable,
} from "../../src/core/infrastructure/portal/DSSPPortalAdapter";
import type { TrainingSession } from "../../src/core/domain/TrainingSession";
import { trainee } from "./FakePortalAdapter";

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("isSupportedTraineePage", () => {
  it("accepts the trainee list with pagination query parameters", () => {
    expect(
      isSupportedTraineePage(
        "https://dssp.frsc.gov.ng/Trainee?pgsize=100&page=1&keywords=",
      ),
    ).toBe(true);
  });

  it("accepts the trainee list without query parameters", () => {
    expect(isSupportedTraineePage("https://dssp.frsc.gov.ng/Trainee")).toBe(
      true,
    );
  });

  it.each([
    "https://dssp.frsc.gov.ng/Training",
    "https://dssp.frsc.gov.ng/Trainee/Delete?TraineeId=5578387",
    "https://example.com/Trainee",
    "not a URL",
  ])("rejects unsupported page %s", (href) => {
    expect(isSupportedTraineePage(href)).toBe(false);
  });
});

describe("isSupportedPortalPage", () => {
  it("accepts the authenticated Enrolled Trainees route", () => {
    expect(isSupportedPortalPage("https://dssp.frsc.gov.ng/Trainee")).toBe(true);
  });

  it("accepts the Log New Training form", () => {
    expect(
      isSupportedPortalPage(
        "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=5578387",
      ),
    ).toBe(true);
  });

  it("rejects the DSSP login page", () => {
    expect(
      isSupportedPortalPage("https://dssp.frsc.gov.ng/Account/Login"),
    ).toBe(false);
  });
});

describe("isAuthenticatedTraineePage", () => {
  it("recognizes /Trainee from its enrolled-trainees DOM", () => {
    const document = {
      readyState: "complete",
      querySelector: (selector: string) =>
        selector === "table.table-checkable tbody" ? {} : null,
      querySelectorAll: () => [],
    } as unknown as Document;

    expect(
      isAuthenticatedTraineePage(document, "https://dssp.frsc.gov.ng/Trainee"),
    ).toBe(true);
  });

  it("rejects the login page even when it is on the DSSP origin", () => {
    const document = {
      readyState: "complete",
      querySelector: (selector: string) =>
        selector === "#loginForm, form[action*=\"/Account/Login\"]" ? {} : null,
      querySelectorAll: () => [],
    } as unknown as Document;

    expect(
      isAuthenticatedTraineePage(
        document,
        "https://dssp.frsc.gov.ng/Account/Login",
      ),
    ).toBe(false);
  });
});

describe("getTrainees", () => {
  it("scrapes the visible trainee table rows", () => {
    const values = [
      "1",
      "12/08/2026",
      "Olubodun Rukayat Tiwalola",
      "01/01/1990",
      "Class B",
      "08012345678",
      "trainee@example.com",
      "3",
      "88",
      "12/08/2026",
      "Admin",
      "",
    ];
    const cells = values.map((textContent) => ({ textContent }));
    const link = {
      href: "https://dssp.frsc.gov.ng/Trainee/Delete?TraineeId=5578387",
    };
    const row = {
      querySelectorAll: () => cells,
      querySelector: () => link,
    };
    const document = {
      querySelectorAll: () => [row],
    } as unknown as Document;

    expect(getTrainees(document)).toEqual([
      {
        id: "5578387",
        traineeId: "5578387",
        sn: "1",
        applicationDate: "12/08/2026",
        name: "Olubodun Rukayat Tiwalola",
        dob: "01/01/1990",
        course: "Class B",
        phone: "08012345678",
        email: "trainee@example.com",
        trainingSessions: "3",
        assessmentScore: "88",
        lastModified: "12/08/2026",
        modifiedBy: "Admin",
        profileUrl: "https://dssp.frsc.gov.ng/Trainee/Delete?TraineeId=5578387",
      },
    ]);
  });

  it("ignores rows without a trainee ID link", () => {
    const row = {
      querySelectorAll: () => [],
      querySelector: () => null,
    };
    const document = {
      querySelectorAll: () => [row],
    } as unknown as Document;

    expect(getTrainees(document)).toEqual([]);
  });
});

function fakeOption(value: string, label: string): HTMLOptionElement {
  return {
    value,
    label,
    textContent: label,
    disabled: false,
    selected: false,
  } as unknown as HTMLOptionElement;
}

function fakeSelect(
  name: string,
  options: HTMLOptionElement[],
  form: HTMLFormElement | null = null,
): HTMLSelectElement {
  return {
    id: name,
    name,
    tagName: "SELECT",
    options,
    value: "",
    form,
    dispatchEvent: vi.fn(() => true),
  } as unknown as HTMLSelectElement;
}

function formDocument(
  instructor: HTMLSelectElement,
  trainingType: HTMLSelectElement,
  trainingDate?: HTMLInputElement,
): Document {
  return {
    querySelector: (selector: string) => {
      if (selector === "select#Instructor") return instructor;
      if (selector === "select#TrainingType") return trainingType;
      if (selector === "#TrainingDate") return trainingDate ?? null;
      return null;
    },
    querySelectorAll: () => [],
  } as unknown as Document;
}

describe("getFormOptions", () => {
  it("does not accept a markerless page while authenticated DOM is still unavailable", async () => {
    vi.useFakeTimers();
    const disconnect = vi.fn();
    vi.stubGlobal(
      "MutationObserver",
      class {
        observe(): void {}
        disconnect(): void {
          disconnect();
        }
      },
    );
    const document = {
      documentElement: {},
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    } as unknown as Document;

    const pending = waitForAuthenticatedTraineePage(
      document,
      "https://dssp.frsc.gov.ng/Trainee",
      50,
    );
    await vi.advanceTimersByTimeAsync(50);

    await expect(pending).resolves.toBe(false);
    expect(disconnect).toHaveBeenCalled();
  });

  it("reports a missing Instructor selector instead of returning empty options", () => {
    const document = formDocument(
      null as unknown as HTMLSelectElement,
      fakeSelect("TrainingType", [fakeOption("classroom", "Classroom")]),
    );

    expect(() => getFormOptions(document)).toThrow(
      "Portal element not found: Instructor select",
    );
  });

  it("preserves every live instructor option without truncation", () => {
    const instructorOptions = Array.from({ length: 150 }, (_, index) =>
      fakeOption(String(index + 1), `INSTRUCTOR ${index + 1}`),
    );
    const document = formDocument(
      fakeSelect("Instructor", instructorOptions),
      fakeSelect("TrainingType", [
        fakeOption("classroom", "Daily training session – Classroom"),
      ]),
    );

    const options = getFormOptions(document);

    expect(options.instructors).toHaveLength(150);
    expect(options.instructors.at(-1)).toEqual({
      value: "150",
      label: "INSTRUCTOR 150",
    });
  });

  it("preserves and exactly matches the training type dash character", () => {
    const exact = "Daily training session – Classroom";
    const trainingType = fakeSelect("TrainingType", [fakeOption(exact, exact)]);
    const document = formDocument(
      fakeSelect("Instructor", [fakeOption("11", "ALAO A. AKEEM")]),
      trainingType,
    );

    expect(getFormOptions(document).trainingTypes[0]?.label).toBe(exact);
    expect(selectFormOption(trainingType, exact)).toEqual({
      value: exact,
      label: exact,
    });
    expect(() =>
      selectFormOption(trainingType, "Daily training session - Classroom"),
    ).toThrow();
  });

  it("loads options from /Trainee without navigating into Trainee Logs", async () => {
    const rowDocument = {
      readyState: "complete",
      querySelector: (selector: string) =>
        selector === "table.table-checkable tbody" ? {} : null,
      querySelectorAll: (selector: string) => {
        if (selector !== "table.table-checkable tbody tr") return [];

        return [
          {
            querySelectorAll: () => Array.from({ length: 12 }, () => ({})),
            querySelector: () => ({
              href: "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=5578387",
            }),
          },
        ];
      },
    } as unknown as Document;
    const fetchedDocument = formDocument(
      fakeSelect("Instructor", [fakeOption("11", "ALAO A. AKEEM")]),
      fakeSelect("TrainingType", [
        fakeOption("classroom", "Daily training session – Classroom"),
      ]),
    );
    const fetcher = vi.fn(() =>
      Promise.resolve({
        ok: true,
        status: 200,
        url: "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=5578387",
        text: () => Promise.resolve("FETCHED_FORM"),
      } as Response),
    );
    const adapter = new DSSPPortalAdapter(
      rowDocument,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      fetcher,
      () => fetchedDocument,
    );

    await expect(adapter.getFormOptions()).resolves.toEqual({
      success: true,
      data: {
        instructors: [{ value: "11", label: "ALAO A. AKEEM" }],
        trainingTypes: [
          {
            value: "classroom",
            label: "Daily training session – Classroom",
          },
        ],
      },
    });
    expect(fetcher).toHaveBeenCalledWith(
      "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=5578387",
      expect.objectContaining({ credentials: "include", cache: "no-store" }),
    );
  });

  it("returns a real error when the fetched option controls are unavailable", async () => {
    const rowDocument = {
      readyState: "complete",
      querySelector: (selector: string) =>
        selector === "table.table-checkable tbody" ? {} : null,
      querySelectorAll: (selector: string) =>
        selector === "table.table-checkable tbody tr"
          ? [
              {
                querySelectorAll: () => Array.from({ length: 12 }, () => ({})),
                querySelector: () => ({
                  href: "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=42",
                }),
              },
            ]
          : [],
    } as unknown as Document;
    const emptyForm = {
      querySelector: () => null,
      querySelectorAll: () => [],
    } as unknown as Document;
    const adapter = new DSSPPortalAdapter(
      rowDocument,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      () =>
        Promise.resolve({
          ok: true,
          status: 200,
          url: "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=42",
          text: () => Promise.resolve("FORM_WITHOUT_OPTIONS"),
        } as Response),
      () => emptyForm,
    );

    const result = await adapter.getFormOptions();

    expect(result.success).toBe(false);
    if (!result.success) {
      expect(result.error.message).toMatch(/Instructor select/);
    }
  });
});

describe("fillTrainingFormFields", () => {
  it("uses exact option values and submits a strict date", () => {
    const form = { checkValidity: () => true } as unknown as HTMLFormElement;
    const instructor = fakeSelect(
      "Instructor",
      [fakeOption("instructor-7", "ATISO S MUSA")],
      form,
    );
    const trainingType = fakeSelect(
      "TrainingType",
      [fakeOption("classroom-1", "Daily training session – Classroom")],
      form,
    );
    const trainingDate = {
      id: "TrainingDate",
      name: "TrainingDate",
      tagName: "INPUT",
      value: "",
      form,
      dispatchEvent: vi.fn(() => true),
    } as unknown as HTMLInputElement;
    const document = formDocument(instructor, trainingType, trainingDate);
    const session: TrainingSession = {
      traineeId: "5578387",
      instructorId: "instructor-7",
      instructorLabel: "ATISO S MUSA",
      trainingTypeId: "classroom-1",
      trainingTypeLabel: "Daily training session – Classroom",
      trainingDate: "14/08/2026",
    };

    fillTrainingFormFields(document, session);

    expect(trainingDate.value).toBe("2026-08-14");
    expect(instructor.value).toBe("instructor-7");
    expect(trainingType.value).toBe("classroom-1");
  });
});

describe("buildTrainingFormPayload", () => {
  it("keeps strict dates and exact selected option values", () => {
    const exactTrainingType = "Daily training session – Classroom";

    vi.stubGlobal(
      "FormData",
      class {
        *entries(): IterableIterator<[string, string]> {
          yield ["__RequestVerificationToken", "token"];
          yield ["TrainingDate", "2026-08-14"];
          yield ["Instructor", "ATISO S MUSA"];
          yield ["TrainingType", exactTrainingType];
        }
      },
    );

    const form = {
      querySelector: () => null,
    } as unknown as HTMLFormElement;
    const payload = buildTrainingFormPayload(form);

    expect(payload.get("TrainingDate")).toBe("2026-08-14");
    expect(payload.get("Instructor")).toBe("ATISO S MUSA");
    expect(payload.get("TrainingType")).toBe(exactTrainingType);
    expect(payload.get("__RequestVerificationToken")).toBe("token");
  });
});

describe("DSSPPortalAdapter submission", () => {
  it("reuses a ready form for the same trainee but reloads for a different trainee", async () => {
    const form = {
      checkValidity: () => true,
      getAttribute: () => null,
      querySelector: () => null,
    } as unknown as HTMLFormElement;
    const makeDocument = () =>
      formDocument(
        fakeSelect("Instructor", [fakeOption("i", "Instructor")], form),
        fakeSelect("TrainingType", [fakeOption("t", "Training")], form),
        {
          id: "TrainingDate",
          name: "TrainingDate",
          tagName: "INPUT",
          form,
          dispatchEvent: vi.fn(() => true),
        } as unknown as HTMLInputElement,
      );
    const fetcher = vi.fn((input: RequestInfo | URL) =>
      Promise.resolve({
        ok: true,
        status: 200,
        url: String(input),
        text: () => Promise.resolve("FORM"),
      } as Response),
    );
    const adapter = new DSSPPortalAdapter(
      {} as Document,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      fetcher,
      makeDocument,
    );

    await adapter.prepareTrainee(trainee("1"));
    await adapter.prepareTrainee(trainee("1"));
    await adapter.prepareTrainee(trainee("2"));

    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(fetcher.mock.calls.map(([url]) => String(url))).toEqual([
      "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=1",
      "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=2",
    ]);
  });

  it("rejects filling a form loaded for another trainee", async () => {
    const form = {
      checkValidity: () => true,
      getAttribute: () => null,
      querySelector: () => null,
    } as unknown as HTMLFormElement;
    const document = formDocument(
      fakeSelect("Instructor", [fakeOption("i", "Instructor")], form),
      fakeSelect("TrainingType", [fakeOption("t", "Training")], form),
      {
        id: "TrainingDate",
        name: "TrainingDate",
        tagName: "INPUT",
        form,
        dispatchEvent: vi.fn(() => true),
      } as unknown as HTMLInputElement,
    );
    const adapter = new DSSPPortalAdapter(
      {} as Document,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      (input) => Promise.resolve({
        ok: true,
        status: 200,
        url: String(input),
        text: () => Promise.resolve("FORM"),
      } as Response),
      () => document,
    );
    await adapter.prepareTrainee(trainee("1"));
    await adapter.openTrainee(trainee("2"));

    const result = await adapter.fillTrainingForm({
      traineeId: "2",
      trainingDate: "2026-08-14",
      instructorId: "i",
      trainingTypeId: "t",
    });

    expect(result.success).toBe(false);
    if (!result.success) expect(result.error.message).toMatch(/does not belong/);
  });

  it("posts the real form fields and confirms a successful response", async () => {
    const formUrl =
      "https://dssp.frsc.gov.ng/Trainee/TrainingLog/TraineeId=5578387";
    const actionUrl = "https://dssp.frsc.gov.ng/Trainee/TrainingLog";
    const form = {
      checkValidity: () => true,
      getAttribute: (name: string) => {
        if (name === "action") return "/Trainee/TrainingLog";
        if (name === "method") return "post";
        return null;
      },
      querySelector: () => null,
    } as unknown as HTMLFormElement;
    const instructor = fakeSelect(
      "Instructor",
      [fakeOption("instructor-7", "ATISO S MUSA")],
      form,
    );
    const trainingType = fakeSelect(
      "TrainingType",
      [fakeOption("classroom-1", "Daily training session – Classroom")],
      form,
    );
    const trainingDate = {
      id: "TrainingDate",
      name: "TrainingDate",
      tagName: "INPUT",
      value: "",
      form,
      dispatchEvent: vi.fn(() => true),
    } as unknown as HTMLInputElement;
    const trainingDocument = formDocument(
      instructor,
      trainingType,
      trainingDate,
    );
    const successDocument = {
      body: { textContent: "Training logged successfully" },
      querySelector: () => null,
      querySelectorAll: () => [],
    } as unknown as Document;
    const submitted: { body?: URLSearchParams } = {};

    vi.stubGlobal(
      "FormData",
      class {
        *entries(): IterableIterator<[string, string]> {
          yield ["__RequestVerificationToken", "token"];
          yield ["TrainingDate", trainingDate.value];
          yield ["Instructor", instructor.value];
          yield ["TrainingType", trainingType.value];
        }
      },
    );

    const fetcher = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => {
      if (init?.method === "POST") {
        if (init.body instanceof URLSearchParams) {
          submitted.body = init.body;
        }

        return Promise.resolve({
          ok: true,
          status: 200,
          url: actionUrl,
          redirected: false,
          text: () => Promise.resolve("Training logged successfully"),
        } as Response);
      }

      return Promise.resolve({
        ok: true,
        status: 200,
        url: formUrl,
        redirected: false,
        text: () => Promise.resolve("FORM"),
      } as Response);
    });
    const adapter = new DSSPPortalAdapter(
      { querySelector: () => null } as unknown as Document,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      fetcher,
      (html) => (html === "FORM" ? trainingDocument : successDocument),
    );
    const record = trainee("5578387");
    const session: TrainingSession = {
      traineeId: record.id,
      instructorId: "instructor-7",
      instructorLabel: "ATISO S MUSA",
      trainingTypeId: "classroom-1",
      trainingTypeLabel: "Daily training session – Classroom",
      trainingDate: "14/08/2026",
    };

    await expect(adapter.openTrainee(record)).resolves.toEqual({
      success: true,
      data: undefined,
    });
    await expect(adapter.openTrainingForm()).resolves.toEqual({
      success: true,
      data: undefined,
    });
    await expect(adapter.fillTrainingForm(session)).resolves.toEqual({
      success: true,
      data: undefined,
    });
    await expect(adapter.validateTrainingForm()).resolves.toEqual({
      success: true,
      data: undefined,
    });
    await expect(adapter.submitTrainingForm()).resolves.toEqual({
      success: true,
      data: undefined,
    });
    await expect(adapter.waitForSubmissionResult()).resolves.toEqual({
      success: true,
      data: { status: "confirmed", reference: actionUrl },
    });

    expect(submitted.body?.get("TrainingDate")).toBe("2026-08-14");
    expect(submitted.body?.get("Instructor")).toBe("instructor-7");
    expect(submitted.body?.get("TrainingType")).toBe("classroom-1");
    expect(submitted.body?.get("__RequestVerificationToken")).toBe("token");
  });

  it("fetches and submits a fresh anti-forgery token per trainee", async () => {
    const actionUrl = "https://dssp.frsc.gov.ng/Trainee/TrainingLog";
    const tokens = ["token-1", "token-2"];
    const postedTokens: Array<string | null> = [];
    let formIndex = 0;

    vi.stubGlobal(
      "FormData",
      class {
        private readonly form: HTMLFormElement;

        constructor(form: HTMLFormElement) {
          this.form = form;
        }

        *entries(): IterableIterator<[string, string]> {
          const token = (this.form as HTMLFormElement & { token: string })
            .token;
          yield ["__RequestVerificationToken", token];
          yield ["TrainingDate", "2026-08-14"];
          yield ["Instructor", "instructor-7"];
          yield ["TrainingType", "classroom-1"];
        }
      },
    );

    const documents = tokens.map((token) => {
      const form = {
        token,
        checkValidity: () => true,
        getAttribute: (name: string) =>
          name === "action"
            ? "/Trainee/TrainingLog"
            : name === "method"
              ? "post"
              : null,
        querySelector: () => null,
      } as unknown as HTMLFormElement;
      const instructor = fakeSelect(
        "Instructor",
        [fakeOption("instructor-7", "ATISO S MUSA")],
        form,
      );
      const trainingType = fakeSelect(
        "TrainingType",
        [fakeOption("classroom-1", "Daily training session – Classroom")],
        form,
      );
      const trainingDate = {
        id: "TrainingDate",
        name: "TrainingDate",
        tagName: "INPUT",
        value: "",
        form,
        dispatchEvent: vi.fn(() => true),
      } as unknown as HTMLInputElement;

      return formDocument(instructor, trainingType, trainingDate);
    });
    const successDocument = {
      body: { textContent: "Training logged successfully" },
      querySelector: () => null,
      querySelectorAll: () => [],
    } as unknown as Document;
    const fetcher = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => {
      if (init?.method === "POST") {
        postedTokens.push(
          init.body instanceof URLSearchParams
            ? init.body.get("__RequestVerificationToken")
            : null,
        );

        return Promise.resolve({
          ok: true,
          status: 200,
          url: actionUrl,
          redirected: false,
          text: () => Promise.resolve("SUCCESS"),
        } as Response);
      }

      formIndex += 1;
      return Promise.resolve({
        ok: true,
        status: 200,
        url:
          typeof _input === "string"
            ? _input
            : _input instanceof URL
              ? _input.href
              : _input.url,
        redirected: false,
        text: () => Promise.resolve(`FORM_${formIndex}`),
      } as Response);
    });
    const adapter = new DSSPPortalAdapter(
      { querySelector: () => null } as unknown as Document,
      { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      fetcher,
      (html) =>
        html === "SUCCESS"
          ? successDocument
          : documents[Number(html.slice(-1)) - 1]!,
    );
    const session: TrainingSession = {
      traineeId: "",
      instructorId: "instructor-7",
      instructorLabel: "ATISO S MUSA",
      trainingTypeId: "classroom-1",
      trainingTypeLabel: "Daily training session – Classroom",
      trainingDate: "14/08/2026",
    };

    for (const id of ["1", "2"]) {
      await adapter.openTrainee(trainee(id));
      await adapter.openTrainingForm();
      await adapter.fillTrainingForm({ ...session, traineeId: id });
      await adapter.validateTrainingForm();
      await adapter.submitTrainingForm();
    }

    expect(postedTokens).toEqual(tokens);
    expect(fetcher.mock.calls.map((call) => call[1]?.method ?? "GET")).toEqual([
      "GET",
      "POST",
      "GET",
      "POST",
    ]);
  });
});

describe("waitForTraineeTable", () => {
  it("waits until the document and trainee table are ready", async () => {
    vi.useFakeTimers();
    let ready = false;
    const document = {
      get readyState() {
        return ready ? "complete" : "loading";
      },
      documentElement: {},
      querySelector: vi.fn(() => (ready ? {} : null)),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    } as unknown as Document;
    const disconnect = vi.fn();

    vi.stubGlobal(
      "MutationObserver",
      class {
        observe(): void {}
        disconnect(): void {
          disconnect();
        }
      },
    );
    vi.stubGlobal("window", globalThis);

    const waiting = waitForTraineeTable(document, 1_000);

    ready = true;
    await vi.advanceTimersByTimeAsync(100);

    await expect(waiting).resolves.toBe(true);
    expect(disconnect).toHaveBeenCalledOnce();
  });
});
