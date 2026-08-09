export interface QueueTask<T> {
  id: string;
  payload: T;
}

export class TaskQueue<T> {
  private readonly tasks: QueueTask<T>[] = [];

  enqueue(task: QueueTask<T>): void {
    this.tasks.push(task);
  }

  enqueueMany(tasks: QueueTask<T>[]): void {
    this.tasks.push(...tasks);
  }

  dequeue(): QueueTask<T> | undefined {
    return this.tasks.shift();
  }

  peek(): QueueTask<T> | undefined {
    return this.tasks[0];
  }

  get size(): number {
    return this.tasks.length;
  }

  get isEmpty(): boolean {
    return this.tasks.length === 0;
  }

  clear(): void {
    this.tasks.length = 0;
  }
}       