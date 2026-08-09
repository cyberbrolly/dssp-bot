import { describe, expect, it } from "vitest";
import { TaskQueue } from "../../src/core/automation/TaskQueue";

describe("TaskQueue", () => {
  it("starts empty", () => {
    const queue = new TaskQueue<string>();

    expect(queue.size).toBe(0);
    expect(queue.isEmpty).toBe(true);
    expect(queue.dequeue()).toBeUndefined();
    expect(queue.peek()).toBeUndefined();
  });

  it("dequeues in FIFO order", () => {
    const queue = new TaskQueue<string>();

    queue.enqueue({ id: "a", payload: "first" });
    queue.enqueue({ id: "b", payload: "second" });

    expect(queue.size).toBe(2);
    expect(queue.peek()?.id).toBe("a");
    expect(queue.dequeue()?.payload).toBe("first");
    expect(queue.dequeue()?.payload).toBe("second");
    expect(queue.isEmpty).toBe(true);
  });

  it("enqueues many while preserving order", () => {
    const queue = new TaskQueue<number>();

    queue.enqueueMany([
      { id: "1", payload: 1 },
      { id: "2", payload: 2 },
      { id: "3", payload: 3 },
    ]);

    expect(queue.size).toBe(3);
    expect(queue.dequeue()?.payload).toBe(1);
    expect(queue.dequeue()?.payload).toBe(2);
  });

  it("clears all pending tasks", () => {
    const queue = new TaskQueue<string>();

    queue.enqueue({ id: "a", payload: "first" });
    queue.clear();

    expect(queue.isEmpty).toBe(true);
    expect(queue.size).toBe(0);
  });
});
