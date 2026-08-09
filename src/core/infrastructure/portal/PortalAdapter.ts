import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";

export interface PortalAdapter {
  isPortalPage(): boolean;

  getCurrentTrainee(): Result<Trainee>;

  openTrainingForm(): Promise<Result<void>>;

  fillTrainingForm(
    session: TrainingSession,
  ): Promise<Result<void>>;

  submitTrainingForm(): Promise<Result<void>>;

  waitForSubmissionResult(): Promise<Result<void>>;
}