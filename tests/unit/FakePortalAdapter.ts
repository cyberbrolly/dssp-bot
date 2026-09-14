import type {
  PortalAdapter,
  SubmissionOutcome,
} from "../../src/core/infrastructure/portal/PortalAdapter";
import type { Result } from "../../src/core/shared/Result";
import type { Trainee } from "../../src/core/domain/Trainee";
import type { TrainingFormOptions } from "../../src/core/domain/TrainingFormOptions";

const ok: Result<void> = {
  success: true,
  data: undefined,
};

export interface FakePortalOptions {
  trainees?: Trainee[];
  formOptions?: TrainingFormOptions;
  outcomeFor?: (traineeId: string) => SubmissionOutcome;
  failOpenTrainee?: (traineeId: string, attempt: number) => Error | null;
  /**
   * A fixed answer, or a hook receiving the 1-based check count so a test can
   * let the session lapse partway through a batch.
   */
  isPortalPage?: boolean | ((check: number) => boolean);
  onOpenTrainee?: (trainee: Trainee) => void;
  /** Fail the submit call itself. `call` counts every submit on the adapter. */
  failSubmit?: (traineeId: string, call: number) => Error | null;
  /**
   * Fail reading the outcome back. `call` counts confirmation attempts for this
   * trainee, so a hook can fail once and then succeed.
   */
  failConfirm?: (traineeId: string, call: number) => Error | null;
}

export class FakePortalAdapter implements PortalAdapter {
  readonly opened: string[] = [];
  readonly submitted: string[] = [];
  readonly confirmed: string[] = [];
  readonly calls: string[] = [];

  private readonly options: FakePortalOptions;
  private readonly attempts = new Map<string, number>();
  private readonly confirmCalls = new Map<string, number>();
  private current: Trainee | null = null;
  private portalPageChecks = 0;
  sessionInitializations = 0;
  trainingFormOpens = 0;
  sessionReady = true;

  initializeTrainingSession(): Promise<Result<void>> {
    this.sessionInitializations += 1;
    this.calls.push("initializeTrainingSession");
    this.sessionReady = true;
    return Promise.resolve(ok);
  }

  isTrainingSessionReady(): Promise<boolean> {
    this.calls.push("isTrainingSessionReady");
    return Promise.resolve(this.sessionReady);
  }

  constructor(options: FakePortalOptions = {}) {
    this.options = options;
  }

  isPortalPage(): Promise<boolean> {
    this.portalPageChecks += 1;

    if (typeof this.options.isPortalPage === "function") {
      return Promise.resolve(this.options.isPortalPage(this.portalPageChecks));
    }

    return Promise.resolve(this.options.isPortalPage ?? true);
  }

  getTrainees(): Promise<Result<Trainee[]>> {
    this.calls.push("getTrainees");
    return Promise.resolve({
      success: true,
      data: this.options.trainees ?? [],
    });
  }

  getFormOptions(): Promise<Result<TrainingFormOptions>> {
    this.calls.push("getFormOptions");
    return Promise.resolve({
      success: true,
      data: this.options.formOptions ?? { instructors: [], trainingTypes: [] },
    });
  }

  openTraineeLogs(): Promise<Result<void>> {
    this.calls.push("openTraineeLogs");
    return Promise.resolve(ok);
  }

  openTrainee(trainee: Trainee): Promise<Result<void>> {
    const attempt = (this.attempts.get(trainee.id) ?? 0) + 1;

    this.attempts.set(trainee.id, attempt);
    this.current = trainee;
    this.opened.push(trainee.id);
    this.options.onOpenTrainee?.(trainee);

    const failure = this.options.failOpenTrainee?.(trainee.id, attempt);

    if (failure) {
      return Promise.resolve({
        success: false,
        error: failure,
      });
    }

    return Promise.resolve(ok);
  }

  prepareTrainee(trainee: Trainee): Promise<Result<void>> {
    this.calls.push("prepareTrainee");
    return this.openTrainee(trainee);
  }

  openTrainingForm(): Promise<Result<void>> {
    this.trainingFormOpens += 1;
    return Promise.resolve(ok);
  }

  fillTrainingForm(): Promise<Result<void>> {
    return Promise.resolve(ok);
  }

  validateTrainingForm(): Promise<Result<void>> {
    return Promise.resolve(ok);
  }

  submitTrainingForm(): Promise<Result<void>> {
    const id = this.current?.id ?? "";
    const failure = this.options.failSubmit?.(id, this.submitted.length + 1);

    // Recorded even when the hook fails, so a test can prove how many times the
    // engine reached the portal's write path.
    this.submitted.push(id);

    if (failure) {
      return Promise.resolve({
        success: false,
        error: failure,
      });
    }

    return Promise.resolve(ok);
  }

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>> {
    const id = this.current?.id ?? "";
    const call = (this.confirmCalls.get(id) ?? 0) + 1;

    this.confirmCalls.set(id, call);

    const failure = this.options.failConfirm?.(id, call);

    if (failure) {
      return Promise.resolve({
        success: false,
        error: failure,
      });
    }

    this.confirmed.push(id);

    const outcome = this.options.outcomeFor?.(id) ?? {
      status: "confirmed" as const,
    };

    return Promise.resolve({
      success: true,
      data: outcome,
    });
  }
}

export function trainee(id: string): Trainee {
  return {
    id,
    traineeId: id,
    sn: id,
    applicationDate: "",
    name: `Trainee ${id}`,
    dob: "",
    course: "",
    phone: "",
    email: "",
    trainingSessions: "",
    assessmentScore: "",
    lastModified: "",
    modifiedBy: "",
    profileUrl: `https://portal.invalid/trainees/${id}`,
  };
}
