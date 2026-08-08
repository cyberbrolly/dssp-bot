export type AutomationState =
  | "idle"
  | "initializing"
  | "opening"
  | "filling"
  | "validating"
  | "submitting"
  | "waiting"
  | "success"
  | "retrying"
  | "failed"
  | "paused"
  | "stopped";
