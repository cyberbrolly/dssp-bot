import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";

export type SubmissionOutcome =
  | { status: "confirmed"; reference?: string }
  | { status: "duplicate"; message: string }
  | { status: "rejected"; message: string };

export interface PortalAdapter {
  /**
   * Whether the pinned tab is still a usable portal page, checked live.
   *
   * Deliberately async and uncached. A batch runs for minutes and the portal
   * session can lapse at any point in it, so a value read once before the run
   * says nothing about the trainee about to be submitted.
   */
  isPortalPage(): Promise<boolean>;

  getTrainees(): Promise<Result<Trainee[]>>;

  getFormOptions(): Promise<Result<TrainingFormOptions>>;

  openTrainee(trainee: Trainee): Promise<Result<void>>;

  openTrainingForm(): Promise<Result<void>>;

  fillTrainingForm(session: TrainingSession): Promise<Result<void>>;

  validateTrainingForm(): Promise<Result<void>>;

  submitTrainingForm(): Promise<Result<void>>;

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>>;
}
