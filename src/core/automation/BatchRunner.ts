import { AutomationEngine, type BatchTask } from "./AutomationEngine";
import { TraineeNotFoundError } from "../shared/errors";
import type { Result } from "../shared/Result";
import type { Trainee } from "../domain/Trainee";
import type { TrainingSession } from "../domain/TrainingSession";
import type { BatchReport } from "../domain/BatchReport";
import type { PortalAdapter } from "../infrastructure/portal/PortalAdapter";

export interface BatchRequest {
  traineeIds: string[];
  session: Omit<TrainingSession, "traineeId">;
}

export class BatchRunner {
  private readonly portal: PortalAdapter;
  private readonly engine: AutomationEngine;

  constructor(portal: PortalAdapter, engine: AutomationEngine) {
    this.portal = portal;
    this.engine = engine;
  }

  async start(request: BatchRequest): Promise<Result<BatchReport>> {
    const trainees = await this.portal.getTrainees();

    if (!trainees.success) {
      return trainees;
    }

    const tasks = this.buildTasks(trainees.data, request);

    if (!tasks.success) {
      return tasks;
    }

    this.engine.clearQueue();
    this.engine.addTasks(tasks.data);

    return this.engine.run();
  }

  private buildTasks(
    available: Trainee[],
    request: BatchRequest,
  ): Result<BatchTask[]> {
    const byId = new Map(available.map((trainee) => [trainee.id, trainee]));

    const tasks: BatchTask[] = [];

    for (const traineeId of request.traineeIds) {
      const trainee = byId.get(traineeId);

      if (!trainee) {
        return {
          success: false,
          error: new TraineeNotFoundError(traineeId),
        };
      }

      tasks.push({
        trainee,
        session: request.session,
      });
    }

    return { success: true, data: tasks };
  }
}
