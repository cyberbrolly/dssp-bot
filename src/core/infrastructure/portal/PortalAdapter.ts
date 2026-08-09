import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";

export type SubmissionOutcome =
  | { status: "confirmed"; reference?: string }
  | { status: "duplicate"; message: string }
  | { status: "rejected"; message: string };

export interface PortalAdapter {
  isPortalPage(): boolean;

  getTrainees(): Promise<Result<Trainee[]>>;

  openTrainee(trainee: Trainee): Promise<Result<void>>;

  openTrainingForm(): Promise<Result<void>>;

  fillTrainingForm(session: TrainingSession): Promise<Result<void>>;

  validateTrainingForm(): Promise<Result<void>>;

  submitTrainingForm(): Promise<Result<void>>;

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>>;
}
