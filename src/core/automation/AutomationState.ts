export type AutomationState =
  | "idle"
  | "initializing"
  | "loading_trainee"
  | "opening_form"
  | "filling_form"
  | "validating"
  | "submitting"
  | "verifying"
  | "retrying"
  | "paused"
  | "stopped"
  | "complete";

export const ACTIVE_STATES: readonly AutomationState[] = [
  "initializing",
  "loading_trainee",
  "opening_form",
  "filling_form",
  "validating",
  "submitting",
  "verifying",
  "retrying",
];

function fromActive(...next: AutomationState[]): readonly AutomationState[] {
  return [
    ...next,
    "retrying",
    "loading_trainee",
    "paused",
    "stopped",
    "complete",
  ];
}

const TRANSITIONS: Record<AutomationState, readonly AutomationState[]> = {
  idle: ["initializing"],
  initializing: fromActive(),
  loading_trainee: fromActive("opening_form"),
  opening_form: fromActive("filling_form"),
  filling_form: fromActive("validating"),
  validating: fromActive("submitting"),
  submitting: fromActive("verifying"),
  verifying: fromActive("loading_trainee"),
  retrying: fromActive(
    "opening_form",
    "filling_form",
    "validating",
    "submitting",
    "verifying",
  ),
  paused: ["loading_trainee", "stopped", "complete"],
  stopped: ["idle"],
  complete: ["idle"],
};

export function isActive(state: AutomationState): boolean {
  return ACTIVE_STATES.includes(state);
}

export function canTransition(
  from: AutomationState,
  to: AutomationState,
): boolean {
  return TRANSITIONS[from].includes(to);
}

export class StateMachine {
  private current: AutomationState = "idle";

  get state(): AutomationState {
    return this.current;
  }

  get isActive(): boolean {
    return isActive(this.current);
  }

  canTransitionTo(next: AutomationState): boolean {
    return canTransition(this.current, next);
  }

  transitionTo(next: AutomationState): void {
    if (!this.canTransitionTo(next)) {
      throw new Error(`Illegal state transition: ${this.current} -> ${next}`);
    }

    this.current = next;
  }

  reset(): void {
    this.current = "idle";
  }
}
