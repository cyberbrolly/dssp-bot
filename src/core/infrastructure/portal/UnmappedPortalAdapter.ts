import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { PortalAdapter, SubmissionOutcome } from "./PortalAdapter";
import { PortalNotMappedError } from "../../shared/errors";

function unmapped<T>(operation: string): Result<T> {
  return {
    success: false,
    error: new PortalNotMappedError(operation),
  };
}

export class UnmappedPortalAdapter implements PortalAdapter {
  isPortalPage(): boolean {
    return false;
  }

  getTrainees(): Promise<Result<Trainee[]>> {
    return Promise.resolve(unmapped<Trainee[]>("getTrainees"));
  }

  openTrainee(): Promise<Result<void>> {
    return Promise.resolve(unmapped<void>("openTrainee"));
  }

  openTrainingForm(): Promise<Result<void>> {
    return Promise.resolve(unmapped<void>("openTrainingForm"));
  }

  fillTrainingForm(_session: TrainingSession): Promise<Result<void>> {
    return Promise.resolve(unmapped<void>("fillTrainingForm"));
  }

  validateTrainingForm(): Promise<Result<void>> {
    return Promise.resolve(unmapped<void>("validateTrainingForm"));
  }

  submitTrainingForm(): Promise<Result<void>> {
    return Promise.resolve(unmapped<void>("submitTrainingForm"));
  }

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>> {
    return Promise.resolve(
      unmapped<SubmissionOutcome>("waitForSubmissionResult"),
    );
  }
}
