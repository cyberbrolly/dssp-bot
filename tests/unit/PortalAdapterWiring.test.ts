import { describe, expect, it } from "vitest";

import {
  PORTAL_COMMAND_METHODS,
  isPortalCommand,
  type PortalCommand,
  type PortalCommandType,
} from "../../src/core/infrastructure/portal/PortalCommands";
import { RemotePortalAdapter } from "../../src/core/infrastructure/portal/RemotePortalAdapter";
import { UnmappedPortalAdapter } from "../../src/core/infrastructure/portal/UnmappedPortalAdapter";
import { DSSPPortalAdapter } from "../../src/core/infrastructure/portal/DSSPPortalAdapter";
import { FakePortalAdapter, trainee } from "./FakePortalAdapter";
import type { PortalAdapter } from "../../src/core/infrastructure/portal/PortalAdapter";
import type { TrainingSession } from "../../src/core/domain/TrainingSession";

/**
 * Guards the popup -> worker -> content script -> DOM boundary.
 *
 * The session operations were once optional on `PortalAdapter` and absent from
 * `RemotePortalAdapter`, the wire protocol, and the content script all at once.
 * Every trainee failed at runtime while all 155 unit tests stayed green, because
 * the only coverage exercised `DSSPPortalAdapter` directly and never crossed the
 * remote hop. These tests assert the correspondence itself rather than any one
 * operation.
 */

/** Every operation the engine may call. Adding one here is deliberate. */
const PORTAL_ADAPTER_METHODS = [
  "initializeTrainingSession",
  "isTrainingSessionReady",
  "isPortalPage",
  "getTrainees",
  "getFormOptions",
  "openTraineeLogs",
  "openTrainee",
  "prepareTrainee",
  "openTrainingForm",
  "fillTrainingForm",
  "validateTrainingForm",
  "submitTrainingForm",
  "waitForSubmissionResult",
] as const satisfies readonly (keyof PortalAdapter)[];

/**
 * Fails to compile if `PortalAdapter` gains a member absent from the list above,
 * which is what keeps the runtime assertions below honest.
 */
type Unlisted = Exclude<
  keyof PortalAdapter,
  (typeof PORTAL_ADAPTER_METHODS)[number]
>;
const _everyMethodListed: Unlisted extends never ? true : Unlisted = true;
void _everyMethodListed;

const ADAPTERS: ReadonlyArray<[string, () => PortalAdapter]> = [
  ["RemotePortalAdapter", () => new RemotePortalAdapter({} as never)],
  ["UnmappedPortalAdapter", () => new UnmappedPortalAdapter()],
  ["FakePortalAdapter", () => new FakePortalAdapter()],
  [
    "DSSPPortalAdapter",
    () =>
      new DSSPPortalAdapter(
        {} as Document,
        { href: "https://dssp.frsc.gov.ng/Trainee" } as Location,
      ),
  ],
];

describe("PortalAdapter implementations", () => {
  it.each(ADAPTERS)("%s implements every operation", (_name, build) => {
    const adapter = build() as unknown as Record<string, unknown>;

    for (const method of PORTAL_ADAPTER_METHODS) {
      expect(typeof adapter[method]).toBe("function");
    }
  });

  it("has no optional members, so a missing operation cannot typecheck", () => {
    // A required member is not assignable from `undefined`. If any operation
    // were made optional again, `Required<PortalAdapter>` would differ from
    // `PortalAdapter` and this equivalence would fail to compile.
    type Equivalent =
      PortalAdapter extends Required<PortalAdapter>
        ? Required<PortalAdapter> extends PortalAdapter
          ? true
          : false
        : false;

    const allRequired: Equivalent = true;

    expect(allRequired).toBe(true);
  });
});

describe("wire protocol", () => {
  it("maps every adapter operation to exactly one command", () => {
    const mapped = Object.values(PORTAL_COMMAND_METHODS);

    expect([...mapped].sort()).toEqual([...PORTAL_ADAPTER_METHODS].sort());
    expect(new Set(mapped).size).toBe(mapped.length);
  });

  it("routes every command through isPortalCommand", () => {
    for (const type of Object.keys(PORTAL_COMMAND_METHODS)) {
      expect(isPortalCommand({ type })).toBe(true);
    }
  });

  it("rejects messages that are not portal commands", () => {
    for (const message of [
      { type: "GET_STATUS" },
      { type: "START_AUTOMATION" },
      { notAType: true },
      null,
      "PORTAL_IS_PAGE",
    ]) {
      expect(isPortalCommand(message)).toBe(false);
    }
  });
});

describe("RemotePortalAdapter forwarding", () => {
  const session: TrainingSession = {
    traineeId: "1",
    trainingDate: "2026-08-14",
    instructorId: "i",
    trainingTypeId: "t",
  };

  /** One representative command per adapter operation. */
  const COMMANDS: ReadonlyArray<[keyof PortalAdapter, PortalCommand]> = [
    ["isPortalPage", { type: "PORTAL_IS_PAGE" }],
    ["initializeTrainingSession", { type: "PORTAL_INITIALIZE_SESSION" }],
    ["isTrainingSessionReady", { type: "PORTAL_SESSION_READY" }],
    ["getTrainees", { type: "PORTAL_GET_TRAINEES" }],
    ["getFormOptions", { type: "PORTAL_GET_FORM_OPTIONS" }],
    ["openTraineeLogs", { type: "PORTAL_OPEN_TRAINEE_LOGS" }],
    ["openTrainee", { type: "PORTAL_OPEN_TRAINEE", trainee: trainee("1") }],
    [
      "prepareTrainee",
      { type: "PORTAL_PREPARE_TRAINEE", trainee: trainee("1") },
    ],
    ["openTrainingForm", { type: "PORTAL_OPEN_FORM" }],
    ["fillTrainingForm", { type: "PORTAL_FILL_FORM", session }],
    ["validateTrainingForm", { type: "PORTAL_VALIDATE_FORM" }],
    ["submitTrainingForm", { type: "PORTAL_SUBMIT_FORM" }],
    ["waitForSubmissionResult", { type: "PORTAL_WAIT_RESULT" }],
  ];

  it("covers every adapter operation", () => {
    expect(COMMANDS.map(([method]) => method).sort()).toEqual(
      [...PORTAL_ADAPTER_METHODS].sort(),
    );
  });

  it.each(COMMANDS)("sends %s as its mapped command", async (method, expected) => {
    const sent: unknown[] = [];
    const adapter = new RemotePortalAdapter({
      findDsspTraineeTab: () => Promise.resolve({ id: 7 }),
      getActiveTab: () => Promise.resolve({ id: 7 }),
      getActiveTabId: () => Promise.resolve(7),
      sendMessage: (_tabId: number, message: unknown) => {
        sent.push(message);
        return Promise.resolve({ success: true, data: true });
      },
    } as never);

    await adapter.attach();

    const call = (adapter as unknown as Record<string, (...args: unknown[]) => unknown>)[
      method
    ];
    await call.call(adapter, trainee("1"), session);

    expect(sent).toHaveLength(1);
    expect(sent[0]).toEqual(expected);
    expect(PORTAL_COMMAND_METHODS[(expected as { type: PortalCommandType }).type]).toBe(
      method,
    );
  });

  it("reports an unready session rather than throwing when the tab is gone", async () => {
    const adapter = new RemotePortalAdapter({
      findDsspTraineeTab: () => Promise.resolve(undefined),
      getActiveTab: () => Promise.resolve(undefined),
      getActiveTabId: () => Promise.resolve(undefined),
      sendMessage: () => Promise.reject(new Error("no receiver")),
    } as never);

    // Never attached: dispatch fails rather than guessing at a target.
    await expect(adapter.isTrainingSessionReady()).resolves.toBe(false);
    await expect(adapter.isPortalPage()).resolves.toBe(false);

    const initialized = await adapter.initializeTrainingSession();

    expect(initialized.success).toBe(false);
  });
});
