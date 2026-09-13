import { AutomationEngine, type BatchTask } from "./AutomationEngine";
import { TraineeNotFoundError } from "../shared/errors";
import type { Result } from "../shared/Result";
import type { Trainee } from "../domain/Trainee";
import type { TrainingSession } from "../domain/TrainingSession";
import type { BatchReport } from "../domain/BatchReport";
import type { PortalAdapter } from "../infrastructure/portal/PortalAdapter";
import { normalizeName } from "../shared/normalizeName";
import { formatTrainingDate } from "../shared/trainingDate";
import { toAutomationError } from "../shared/errors";

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
    let normalizedRequest: BatchRequest;

    try {
      normalizedRequest = {
        ...request,
        session: {
          ...request.session,
          trainingDate: formatTrainingDate(request.session.trainingDate),
        },
      };
    } catch (error) {
      return { success: false, error: toAutomationError(error) };
    }

    const trainees = await this.portal.getTrainees();

    if (!trainees.success) {
      return trainees;
    }

    const tasks = this.buildTasks(trainees.data, normalizedRequest);

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
    console.debug("[DSSP:BatchRunner] trainee resolution snapshot", {
      inputIds: request.traineeIds,
      availableLength: available.length,
      names: available.map((trainee) => trainee.name),
      normalizedNames: available.map((trainee) => normalizeName(trainee.name)),
      targetNameDiagnostics: available
        .filter(
          (trainee) => normalizeName(trainee.name) === "ogbonna victor chinoso",
        )
        .map((trainee) => ({
          rawName: trainee.name,
          rawCodePoints: Array.from(trainee.name, (char) =>
            char.codePointAt(0),
          ),
        })),
    });
    const byId = new Map(available.map((trainee) => [trainee.id, trainee]));
    const byName = new Map(
      available.map((trainee) => [normalizeName(trainee.name), trainee]),
    );

    const tasks: BatchTask[] = [];

    for (const traineeInput of request.traineeIds) {
      const input = traineeInput.trim();
      const normalizedInput = normalizeName(input);
      console.debug("[DSSP:BatchRunner] resolving trainee input", {
        rawInput: traineeInput,
        trimmedInput: input,
        normalizedInput,
        numeric: /^\d+$/.test(input),
      });
      const trainee = /^\d+$/.test(input)
        ? byId.get(input)
        : byName.get(normalizedInput);

      console.debug("[DSSP:BatchRunner] resolution result", {
        rawInput: traineeInput,
        normalizedInput,
        matched: trainee ? { id: trainee.id, name: trainee.name } : null,
      });

      if (!trainee && !/^\d+$/.test(input)) {
        return {
          success: false,
          error: new TraineeNotFoundError(traineeInput),
        };
      }

      tasks.push({
        trainee: trainee ?? {
          id: input,
          traineeId: input,
          sn: "",
          applicationDate: "",
          name: `Trainee ${input}`,
          dob: "",
          course: "",
          phone: "",
          email: "",
          trainingSessions: "",
          assessmentScore: "",
          lastModified: "",
          modifiedBy: "",
          profileUrl: "",
        },
        session: request.session,
      });
    }

    return { success: true, data: tasks };
  }
}
