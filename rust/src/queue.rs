//! FIFO task queue for batch runs. Port of TaskQueue.ts.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct QueueTask<T> {
    pub id: String,
    pub payload: T,
}

/// A plain first-in, first-out queue of identified tasks.
#[derive(Debug)]
pub struct TaskQueue<T> {
    tasks: VecDeque<QueueTask<T>>,
}

impl<T> Default for TaskQueue<T> {
    fn default() -> Self {
        Self {
            tasks: VecDeque::new(),
        }
    }
}

impl<T> TaskQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, id: impl Into<String>, payload: T) {
        self.tasks.push_back(QueueTask {
            id: id.into(),
            payload,
        });
    }

    pub fn dequeue(&mut self) -> Option<QueueTask<T>> {
        self.tasks.pop_front()
    }

    /// Put a dequeued task back at the front, where it came from.
    ///
    /// For the one case a task leaves the queue and must not be run: the run
    /// stopped before submitting it. At the front rather than the back, so the
    /// order a batch was queued in survives — a drain records these as skipped
    /// in dequeue order, and that order is what a resume re-runs them in.
    pub fn put_back(&mut self, task: QueueTask<T>) {
        self.tasks.push_front(task);
    }

    pub fn peek(&self) -> Option<&QueueTask<T>> {
        self.tasks.front()
    }

    /// Ids of everything still queued, in dequeue order.
    pub fn ids(&self) -> Vec<String> {
        self.tasks.iter().map(|task| task.id.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dequeues_in_fifo_order() {
        let mut q: TaskQueue<i32> = TaskQueue::new();
        q.enqueue("a", 1);
        q.enqueue("b", 2);
        q.enqueue("c", 3);

        assert_eq!(q.len(), 3);
        assert_eq!(q.ids(), vec!["a", "b", "c"]);
        assert_eq!(q.peek().map(|t| t.id.as_str()), Some("a"));

        let first = q.dequeue().unwrap();
        assert_eq!(first.id, "a");
        assert_eq!(first.payload, 1);
        assert_eq!(q.ids(), vec!["b", "c"]);
    }

    /// The trainee a stopped run took off the queue but never sent belongs back
    /// where it was, not at the end: the order these are drained in is the order
    /// a resume re-runs them in.
    #[test]
    fn a_task_put_back_keeps_its_place() {
        let mut q: TaskQueue<i32> = TaskQueue::new();
        q.enqueue("a", 1);
        q.enqueue("b", 2);
        q.enqueue("c", 3);

        let taken = q.dequeue().unwrap();
        assert_eq!(taken.id, "a");

        q.put_back(taken);

        assert_eq!(q.ids(), vec!["a", "b", "c"]);
        assert_eq!(q.dequeue().unwrap().payload, 1);
        assert_eq!(q.ids(), vec!["b", "c"]);
    }

    #[test]
    fn empty_and_clear() {
        let mut q: TaskQueue<&str> = TaskQueue::new();
        assert!(q.is_empty());
        assert!(q.dequeue().is_none());

        q.enqueue("x", "payload");
        assert!(!q.is_empty());
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.ids(), Vec::<String>::new());
    }
}
