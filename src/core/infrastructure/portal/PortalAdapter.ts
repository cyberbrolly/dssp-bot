import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";

export type SubmissionOutcome =
  | { status: "confirmed"; reference?: string }
  | { status: "duplicate"; message: string }
  | { status: "rejected"; message: string };

/**
 * Every operation the engine can ask of the DSSP portal.
 *
 * Deliberately free of optional members. The three session operations were once
 * declared `?:` so that adapters could opt in, and `RemotePortalAdapter` — the
 * only adapter the service worker actually uses — silently implemented none of
 * them while still satisfying this interface. The engine feature-detected them,
 * found nothing, and fell through to a path that failed every trainee. Nothing
 * caught it, because optionality made the omission legal.
 *
 * A required member turns that omission back into a compile error. Any adapter
 * that genuinely cannot perform an operation says so at runtime, by returning a
 * failed `Result` the way `UnmappedPortalAdapter` does.
 */
export interface PortalAdapter {
  /**
   * Establish the batch-level training session, once per batch.
   *
   * Validates that the portal is reachable and that the configured training
   * details resolve against the live form, so a misconfiguration fails before
   * the first record is written rather than after.
   */
  initializeTrainingSession(): Promise<Result<void>>;

  /**
   * Whether the session established above is still usable.
   *
   * Checked per trainee. A `false` answer means the engine reinitialises rather
   * than submitting against a lapsed session.
   */
  isTrainingSessionReady(): Promise<boolean>;

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

  openTraineeLogs(): Promise<Result<void>>;

  openTrainee(trainee: Trainee): Promise<Result<void>>;

  /**
   * Prepare the next trainee while retaining the batch-level session.
   *
   * Preferred over `openTrainee` + `openTrainingForm`: it reuses the prepared
   * form when it already belongs to this trainee, which is what makes the
   * session persistent rather than rebuilt per trainee.
   */
  prepareTrainee(trainee: Trainee): Promise<Result<void>>;

  openTrainingForm(): Promise<Result<void>>;

  fillTrainingForm(session: TrainingSession): Promise<Result<void>>;

  validateTrainingForm(): Promise<Result<void>>;

  submitTrainingForm(): Promise<Result<void>>;

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>>;
}
