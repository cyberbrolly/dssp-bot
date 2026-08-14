import type { Trainee } from "../../domain/Trainee";
import type {
  TrainingFormOption,
  TrainingFormOptions,
} from "../../domain/TrainingFormOptions";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { Result } from "../../shared/Result";
import {
  ConfirmationUnknownError,
  MissingDataError,
  NetworkError,
  PortalElementNotFoundError,
  PortalStructureError,
  SessionExpiredError,
  TimeoutError,
  ValidationError,
  toAutomationError,
} from "../../shared/errors";
import { formatTrainingDate } from "../../shared/trainingDate";
import type { PortalAdapter, SubmissionOutcome } from "./PortalAdapter";

const TRAINEE_ROW_SELECTOR = "table.table-checkable tbody tr";
const TRAINEE_ID_LINK_SELECTOR = 'a[href*="TraineeId="]';
const TRAINING_LOG_LINK_SELECTOR =
  'a[href*="/Trainee/TrainingLog/"][href*="TraineeId="]';
const TRAINEE_TABLE_SELECTOR = "table.table-checkable tbody";
const LOGIN_FORM_SELECTOR = '#loginForm, form[action*="/Account/Login"]';
const VALIDATION_MESSAGE_SELECTOR =
  ".validation-summary-errors, .field-validation-error";
const SUBMIT_SELECTOR = 'button[type="submit"], input[type="submit"]';
const TRAINEE_PAGE_PATH = "/Trainee";
const TRAINING_FORM_PATH = "/Trainee/TrainingLog/TraineeId=";
const DSSP_ORIGIN = "https://dssp.frsc.gov.ng";
const TRAINEE_LIST_URL = `${DSSP_ORIGIN}/Trainee?pgsize=10000&page=1&keywords=`;
const TABLE_READY_TIMEOUT_MS = 10_000;

const TRAINING_DATE_SELECTORS = [
  "#TrainingDate",
  'input[name="TrainingDate"]',
  "#Date",
  'input[name="Date"]',
] as const;

const INSTRUCTOR_SELECTORS = [
  "select#Instructor",
  'select[name="Instructor"]',
  "select#InstructorId",
  'select[name="InstructorId"]',
  "select#InstructorID",
  'select[name="InstructorID"]',
] as const;

const TRAINING_TYPE_SELECTORS = [
  "select#TrainingType",
  'select[name="TrainingType"]',
  "select#TrainingTypeId",
  'select[name="TrainingTypeId"]',
  "select#TrainingTypeID",
  'select[name="TrainingTypeID"]',
] as const;

type PortalFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

interface LoadedDocument {
  document: Document;
  url: string;
  body: string;
}

interface TrainingFormFields {
  form: HTMLFormElement;
  trainingDate: HTMLInputElement;
  instructor: HTMLSelectElement;
  trainingType: HTMLSelectElement;
}

interface PreparedTrainingForm extends LoadedDocument {
  fields: TrainingFormFields;
}

interface SubmissionSnapshot {
  status: number;
  url: string;
  body: string;
  redirected: boolean;
}

function ok<T>(data: T): Result<T> {
  return { success: true, data };
}

function failed<T>(error: unknown): Result<T> {
  return { success: false, error: toAutomationError(error) };
}

function trainingFormUrl(traineeId: string): string {
  return `${DSSP_ORIGIN}${TRAINING_FORM_PATH}${encodeURIComponent(traineeId)}`;
}

function normalizedPath(url: URL): string {
  return url.pathname.replace(/\/+$/, "") || "/";
}

export function isSupportedTraineePage(href: string): boolean {
  try {
    const url = new URL(href);

    return (
      url.origin === DSSP_ORIGIN && normalizedPath(url) === TRAINEE_PAGE_PATH
    );
  } catch {
    return false;
  }
}

export function isSupportedPortalPage(href: string): boolean {
  try {
    const url = new URL(href);
    const pathname = normalizedPath(url);

    return (
      url.origin === DSSP_ORIGIN &&
      (pathname === TRAINEE_PAGE_PATH ||
        pathname.startsWith(`${TRAINEE_PAGE_PATH}/`))
    );
  } catch {
    return false;
  }
}

export async function waitForTraineeTable(
  document: Document,
  timeoutMs = TABLE_READY_TIMEOUT_MS,
): Promise<boolean> {
  const isReady = (): boolean =>
    document.readyState === "complete" &&
    document.querySelector(TRAINEE_TABLE_SELECTOR) !== null;

  if (isReady()) {
    return true;
  }

  return new Promise<boolean>((resolve) => {
    const finish = (ready: boolean): void => {
      clearTimeout(timeoutId);
      observer.disconnect();
      document.removeEventListener("readystatechange", check);
      clearInterval(intervalId);
      resolve(ready);
    };
    const check = (): void => {
      if (isReady()) finish(true);
    };
    const observer = new MutationObserver(check);
    const intervalId = setInterval(check, 100);
    const timeoutId = setTimeout(() => finish(false), timeoutMs);

    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
    document.addEventListener("readystatechange", check);
    check();
  });
}

function text(cells: HTMLTableCellElement[], index: number): string {
  return cells[index]?.textContent?.trim() ?? "";
}

function traineeIdFromHref(href: string): string | undefined {
  return /(?:[?&/])TraineeId=(\d+)/i.exec(href)?.[1];
}

export function getTrainees(document: Document): Trainee[] {
  return Array.from(
    document.querySelectorAll<HTMLTableRowElement>(TRAINEE_ROW_SELECTOR),
  )
    .map((row) => {
      const cells = Array.from(
        row.querySelectorAll<HTMLTableCellElement>("td"),
      );
      const trainingLogLink = row.querySelector<HTMLAnchorElement>(
        TRAINING_LOG_LINK_SELECTOR,
      );
      const idLink = row.querySelector<HTMLAnchorElement>(
        TRAINEE_ID_LINK_SELECTOR,
      );
      const traineeId = traineeIdFromHref(
        trainingLogLink?.href ?? idLink?.href ?? "",
      );

      if (!traineeId) {
        return null;
      }

      return {
        id: traineeId,
        traineeId,
        sn: text(cells, 0),
        applicationDate: text(cells, 1),
        name: text(cells, 2),
        dob: text(cells, 3),
        course: text(cells, 4),
        phone: text(cells, 5),
        email: text(cells, 6),
        trainingSessions: text(cells, 7),
        assessmentScore: text(cells, 8),
        lastModified: text(cells, 9),
        modifiedBy: text(cells, 10),
        profileUrl: trainingLogLink?.href ?? trainingFormUrl(traineeId),
      };
    })
    .filter((trainee): trainee is Trainee => trainee !== null);
}

function queryFirst<T extends Element>(
  root: ParentNode,
  selectors: readonly string[],
): T | null {
  for (const selector of selectors) {
    const element = root.querySelector<T>(selector);

    if (element) {
      return element;
    }
  }

  return null;
}

function labelText(label: HTMLLabelElement): string {
  return label.textContent?.replace(/\s+/g, " ").trim() ?? "";
}

function labelledControl<T extends HTMLElement>(
  document: Document,
  tagName: "input" | "select",
  labelPattern: RegExp,
): T | null {
  const labels = Array.from(
    document.querySelectorAll<HTMLLabelElement>("label"),
  );

  for (const label of labels) {
    if (!labelPattern.test(labelText(label))) {
      continue;
    }

    const labelled = label.htmlFor
      ? document.getElementById(label.htmlFor)
      : null;
    const nested = label.querySelector<HTMLElement>(tagName);
    const nearby = label.parentElement?.querySelector<HTMLElement>(tagName);
    const control = labelled ?? nested ?? nearby ?? null;

    if (control?.tagName.toLowerCase() === tagName) {
      return control as T;
    }
  }

  return null;
}

function tokenControl<T extends HTMLElement>(
  document: Document,
  tagName: "input" | "select",
  tokens: readonly string[],
): T | null {
  return (
    Array.from(document.querySelectorAll<T>(tagName)).find((control) => {
      const identity = `${control.id} ${control.getAttribute("name") ?? ""}`
        .replace(/[^a-z0-9]+/gi, " ")
        .toLowerCase();

      return tokens.every((token) => identity.includes(token));
    }) ?? null
  );
}

function instructorSelect(document: Document): HTMLSelectElement | null {
  return (
    queryFirst<HTMLSelectElement>(document, INSTRUCTOR_SELECTORS) ??
    labelledControl<HTMLSelectElement>(document, "select", /instructor/i) ??
    tokenControl<HTMLSelectElement>(document, "select", ["instructor"])
  );
}

function trainingTypeSelect(document: Document): HTMLSelectElement | null {
  return (
    queryFirst<HTMLSelectElement>(document, TRAINING_TYPE_SELECTORS) ??
    labelledControl<HTMLSelectElement>(
      document,
      "select",
      /training\s*type/i,
    ) ??
    tokenControl<HTMLSelectElement>(document, "select", ["training", "type"])
  );
}

function trainingDateInput(document: Document): HTMLInputElement | null {
  return (
    queryFirst<HTMLInputElement>(document, TRAINING_DATE_SELECTORS) ??
    labelledControl<HTMLInputElement>(
      document,
      "input",
      /training\s*date|date.*yyyy/i,
    ) ??
    tokenControl<HTMLInputElement>(document, "input", ["training", "date"])
  );
}

function optionLabel(option: HTMLOptionElement): string {
  return (option.textContent ?? option.label).trim();
}

export function getSelectOptions(
  select: HTMLSelectElement,
): TrainingFormOption[] {
  return Array.from(select.options)
    .filter(
      (option) =>
        !option.disabled &&
        option.value.trim().length > 0 &&
        optionLabel(option),
    )
    .map((option) => ({
      value: option.value,
      label: optionLabel(option),
    }));
}

export function getFormOptions(document: Document): TrainingFormOptions {
  const instructor = instructorSelect(document);
  const trainingType = trainingTypeSelect(document);

  if (!instructor) {
    throw new PortalElementNotFoundError("Instructor select");
  }

  if (!trainingType) {
    throw new PortalElementNotFoundError("Training Type select");
  }

  const instructors = getSelectOptions(instructor);
  const trainingTypes = getSelectOptions(trainingType);

  if (instructors.length === 0) {
    throw new MissingDataError("Instructor options");
  }

  if (trainingTypes.length === 0) {
    throw new MissingDataError("Training Type options");
  }

  return { instructors, trainingTypes };
}

function emitValueEvents(element: HTMLInputElement | HTMLSelectElement): void {
  element.dispatchEvent(new Event("input", { bubbles: true }));
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

export function selectFormOption(
  select: HTMLSelectElement,
  selectedValue: string,
  selectedLabel?: string,
): TrainingFormOption {
  const options = Array.from(select.options).filter(
    (option) => !option.disabled && option.value.trim().length > 0,
  );
  const matched = options.find((option) => {
    if (selectedLabel !== undefined) {
      return (
        option.value === selectedValue && optionLabel(option) === selectedLabel
      );
    }

    return (
      option.value === selectedValue || optionLabel(option) === selectedValue
    );
  });

  if (!matched) {
    throw new ValidationError(
      `The selected value is not present in ${select.name || select.id}.`,
    );
  }

  for (const option of Array.from(select.options)) {
    option.selected = option === matched;
  }

  select.value = matched.value;
  emitValueEvents(select);

  return { value: matched.value, label: optionLabel(matched) };
}

function associatedForm(
  ...controls: Array<HTMLInputElement | HTMLSelectElement>
): HTMLFormElement | null {
  for (const control of controls) {
    const form = control.form ?? control.closest<HTMLFormElement>("form");

    if (form) {
      return form;
    }
  }

  return null;
}

function getTrainingFormFields(document: Document): TrainingFormFields {
  const trainingDate = trainingDateInput(document);
  const instructor = instructorSelect(document);
  const trainingType = trainingTypeSelect(document);

  if (!trainingDate) {
    throw new PortalElementNotFoundError("Training Date input");
  }

  if (!instructor) {
    throw new PortalElementNotFoundError("Instructor select");
  }

  if (!trainingType) {
    throw new PortalElementNotFoundError("Training Type select");
  }

  const form = associatedForm(trainingDate, instructor, trainingType);

  if (!form) {
    throw new PortalElementNotFoundError("Log New Training form");
  }

  return { form, trainingDate, instructor, trainingType };
}

export function fillTrainingFormFields(
  document: Document,
  session: TrainingSession,
): TrainingFormFields {
  const fields = getTrainingFormFields(document);

  fields.trainingDate.value = formatTrainingDate(session.trainingDate);
  emitValueEvents(fields.trainingDate);
  selectFormOption(
    fields.instructor,
    session.instructorId,
    session.instructorLabel,
  );
  selectFormOption(
    fields.trainingType,
    session.trainingTypeId,
    session.trainingTypeLabel,
  );

  return fields;
}

export function buildTrainingFormPayload(
  form: HTMLFormElement,
): URLSearchParams {
  const submitter = form.querySelector<HTMLElement>(SUBMIT_SELECTOR);
  const formData = submitter
    ? new FormData(form, submitter)
    : new FormData(form);
  const payload = new URLSearchParams();

  for (const [name, value] of formData.entries()) {
    if (typeof value !== "string") {
      throw new PortalStructureError(
        "The training form contains an unsupported file field.",
      );
    }

    payload.append(name, value);
  }

  return payload;
}

function isLoginPage(document: Document, href: string): boolean {
  try {
    if (new URL(href, DSSP_ORIGIN).pathname.startsWith("/Account/Login")) {
      return true;
    }
  } catch {
    return true;
  }

  return document.querySelector(LOGIN_FORM_SELECTOR) !== null;
}

function validationMessage(document: Document): string | null {
  const messages = Array.from(
    document.querySelectorAll<HTMLElement>(VALIDATION_MESSAGE_SELECTOR),
  )
    .map((element) => element.textContent?.replace(/\s+/g, " ").trim() ?? "")
    .filter(Boolean);

  return messages.length > 0 ? messages.join(" ") : null;
}

function messageFromJson(body: string): {
  success?: boolean;
  message?: string;
} | null {
  try {
    const value = JSON.parse(body) as unknown;

    if (typeof value !== "object" || value === null) {
      return null;
    }

    const record = value as Record<string, unknown>;
    const rawSuccess = record["success"] ?? record["Success"];
    const rawMessage =
      record["message"] ??
      record["Message"] ??
      record["error"] ??
      record["Error"];

    return {
      ...(typeof rawSuccess === "boolean" ? { success: rawSuccess } : {}),
      ...(typeof rawMessage === "string" ? { message: rawMessage } : {}),
    };
  } catch {
    return null;
  }
}

function submissionOutcome(
  snapshot: SubmissionSnapshot,
  parseHtml: (html: string) => Document,
): Result<SubmissionOutcome> {
  const json = messageFromJson(snapshot.body);
  const document = parseHtml(snapshot.body);

  if (isLoginPage(document, snapshot.url)) {
    return failed(new SessionExpiredError());
  }

  const visibleText =
    document.body?.textContent?.replace(/\s+/g, " ").trim() ??
    snapshot.body.trim();
  const message = json?.message ?? visibleText;

  if (/duplicate|already\s+(?:logged|recorded|exists?)/i.test(message)) {
    return ok({ status: "duplicate", message });
  }

  const validation = validationMessage(document);

  if (json?.success === false || validation || snapshot.status >= 400) {
    return ok({
      status: "rejected",
      message:
        json?.message ?? validation ?? `DSSP returned HTTP ${snapshot.status}.`,
    });
  }

  if (
    json?.success === true ||
    /success|successfully|saved|recorded|created/i.test(message) ||
    snapshot.status === 204 ||
    (snapshot.status >= 200 &&
      snapshot.status < 300 &&
      snapshot.body.trim().length === 0) ||
    (snapshot.redirected &&
      !new URL(snapshot.url, DSSP_ORIGIN).pathname.includes("/TrainingLog"))
  ) {
    return ok({ status: "confirmed", reference: snapshot.url });
  }

  return failed(
    new ConfirmationUnknownError(
      `DSSP returned HTTP ${snapshot.status} without a recognizable result.`,
    ),
  );
}

export class DSSPPortalAdapter implements PortalAdapter {
  private readonly document: Document;
  private readonly location: Location;
  private readonly fetcher: PortalFetch;
  private readonly parseHtml: (html: string) => Document;

  private currentTrainee: Trainee | null = null;
  private preparedForm: PreparedTrainingForm | null = null;
  private submission: SubmissionSnapshot | null = null;

  constructor(
    document: Document,
    location: Location,
    fetcher: PortalFetch = (input, init) => fetch(input, init),
    parseHtml: (html: string) => Document = (html) =>
      new DOMParser().parseFromString(html, "text/html"),
  ) {
    this.document = document;
    this.location = location;
    this.fetcher = fetcher;
    this.parseHtml = parseHtml;
  }

  isPortalPage(): Promise<boolean> {
    return Promise.resolve(
      isSupportedPortalPage(this.location.href) &&
        !isLoginPage(this.document, this.location.href),
    );
  }

  async getTrainees(): Promise<Result<Trainee[]>> {
    if (isSupportedTraineePage(this.location.href)) {
      if (!(await waitForTraineeTable(this.document))) {
        return failed(
          new TimeoutError(
            "waiting for the trainee table to load",
            TABLE_READY_TIMEOUT_MS,
          ),
        );
      }

      return ok(getTrainees(this.document));
    }

    const loaded = await this.loadDocument(TRAINEE_LIST_URL);

    return loaded.success ? ok(getTrainees(loaded.data.document)) : loaded;
  }

  async getFormOptions(): Promise<Result<TrainingFormOptions>> {
    const trainees = await this.getTrainees();

    if (!trainees.success) {
      return trainees;
    }

    const first = trainees.data[0];

    if (!first) {
      return failed(new MissingDataError("a trainee to load form options"));
    }

    const loaded = await this.loadDocument(trainingFormUrl(first.id));

    if (!loaded.success) {
      return loaded;
    }

    try {
      return ok(getFormOptions(loaded.data.document));
    } catch (error) {
      return failed(error);
    }
  }

  openTrainee(trainee: Trainee): Promise<Result<void>> {
    this.currentTrainee = trainee;
    this.preparedForm = null;
    this.submission = null;

    return Promise.resolve(ok(undefined));
  }

  async openTrainingForm(): Promise<Result<void>> {
    const trainee = this.currentTrainee;

    if (!trainee) {
      return failed(new MissingDataError("trainee"));
    }

    const loaded = await this.loadDocument(trainingFormUrl(trainee.id));

    if (!loaded.success) {
      return loaded;
    }

    try {
      this.preparedForm = {
        ...loaded.data,
        fields: getTrainingFormFields(loaded.data.document),
      };

      return ok(undefined);
    } catch (error) {
      return failed(error);
    }
  }

  fillTrainingForm(session: TrainingSession): Promise<Result<void>> {
    if (!this.preparedForm) {
      return Promise.resolve(failed(new MissingDataError("training form")));
    }

    try {
      this.preparedForm.fields = fillTrainingFormFields(
        this.preparedForm.document,
        session,
      );

      return Promise.resolve(ok(undefined));
    } catch (error) {
      return Promise.resolve(failed(error));
    }
  }

  validateTrainingForm(): Promise<Result<void>> {
    const fields = this.preparedForm?.fields;

    if (!fields) {
      return Promise.resolve(failed(new MissingDataError("training form")));
    }

    try {
      fields.trainingDate.value = formatTrainingDate(fields.trainingDate.value);

      if (
        !fields.trainingDate.name ||
        !fields.instructor.name ||
        !fields.trainingType.name
      ) {
        throw new PortalStructureError(
          "A required training control has no form field name.",
        );
      }

      if (!fields.form.checkValidity()) {
        throw new ValidationError("The DSSP training form is not valid.");
      }

      return Promise.resolve(ok(undefined));
    } catch (error) {
      return Promise.resolve(failed(error));
    }
  }

  async submitTrainingForm(): Promise<Result<void>> {
    const prepared = this.preparedForm;

    if (!prepared) {
      return failed(new MissingDataError("training form"));
    }

    try {
      const action = new URL(
        prepared.fields.form.getAttribute("action") || prepared.url,
        prepared.url,
      );
      const method = (
        prepared.fields.form.getAttribute("method") || "get"
      ).toUpperCase();

      if (method !== "POST") {
        throw new PortalStructureError(
          `The Log New Training form uses unsupported method ${method}.`,
        );
      }

      const response = await this.fetcher(action, {
        method,
        body: buildTrainingFormPayload(prepared.fields.form),
        credentials: "include",
        redirect: "follow",
        headers: {
          "X-Requested-With": "XMLHttpRequest",
        },
      });
      const body = await response.text();
      const url = response.url || action.href;
      const document = this.parseHtml(body);

      if (isLoginPage(document, url)) {
        return failed(new SessionExpiredError());
      }

      this.submission = {
        status: response.status,
        url,
        body,
        redirected: response.redirected,
      };

      return ok(undefined);
    } catch (error) {
      if (
        error instanceof PortalStructureError ||
        error instanceof SessionExpiredError
      ) {
        return failed(error);
      }

      return failed(
        new NetworkError(
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
  }

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>> {
    if (!this.submission) {
      return Promise.resolve(
        failed(new TimeoutError("waiting for the DSSP response", 0)),
      );
    }

    return Promise.resolve(submissionOutcome(this.submission, this.parseHtml));
  }

  private async loadDocument(url: string): Promise<Result<LoadedDocument>> {
    try {
      const response = await this.fetcher(url, {
        credentials: "include",
        redirect: "follow",
        cache: "no-store",
      });
      const body = await response.text();
      const responseUrl = response.url || url;
      const document = this.parseHtml(body);

      if (isLoginPage(document, responseUrl)) {
        return failed(new SessionExpiredError());
      }

      if (!response.ok) {
        return failed(
          new NetworkError(`DSSP returned HTTP ${response.status} for ${url}.`),
        );
      }

      return ok({ document, url: responseUrl, body });
    } catch (error) {
      if (error instanceof SessionExpiredError) {
        return failed(error);
      }

      return failed(
        new NetworkError(
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
  }
}
