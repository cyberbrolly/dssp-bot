import { TaskQueue, type QueueTask } from "./TaskQueue";
import type { AutomationState } from "./AutomationState";
import type { Result } from "../shared/Result";

export interface AutomationTask {
  id: string;
}

export class AutomationEngine {
  private state: AutomationState = "idle";
  private readonly queue = new TaskQueue<AutomationTask>();
  private stopRequested = false;

  getState(): AutomationState {
    return this.state;
  }

  getQueueSize(): number {
    return this.queue.size;
  }

  addTask(task: AutomationTask): void {
    this.queue.enqueue({
      id: task.id,
      payload: task,
    });
  }

  addTasks(tasks: AutomationTask[]): void {
    const queueTasks: QueueTask<AutomationTask>[] = tasks.map((task) => ({
      id: task.id,
      payload: task,
    }));

    this.queue.enqueueMany(queueTasks);
  }

  async run(): Promise<Result<void>> {
    if (this.state === "initializing" || this.state === "waiting") {
      return {
        success: false,
        error: new Error("Automation is already running."),
      };
    }

    this.stopRequested = false;
    this.state = "initializing";

    try {
      while (!this.queue.isEmpty && !this.stopRequested) {
        const task = this.queue.dequeue();

        if (!task) {
          break;
        }

        await this.processTask(task.payload);
      }

      if (this.stopRequested) {
        this.state = "stopped";
      } else {
        this.state = "success";
      }

      return {
        success: true,
        data: undefined,
      };
    } catch (error) {
      this.state = "failed";

      return {
        success: false,
        error: error instanceof Error
          ? error
          : new Error(String(error)),
      };
    }
  }

  pause(): void {
    if (this.state === "initializing" || this.state === "waiting") {
      this.state = "paused";
    }
  }

  stop(): void {
    this.stopRequested = true;
    this.state = "stopped";
  }

  clearQueue(): void {
    this.queue.clear();
  }

  private async processTask(task: AutomationTask): Promise<void> {
    this.state = "initializing";

    console.log(`Processing task: ${task.id}`);

    // Portal-specific automation will be implemented later.
    await Promise.resolve();

    this.state = "waiting";
  }
}