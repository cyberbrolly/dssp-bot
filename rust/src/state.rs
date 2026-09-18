//! Automation state machine. Port of AutomationState.ts.
//!
//! Guards the per-trainee progression so an illegal jump (e.g. submitting
//! without having loaded a trainee) is a hard error rather than a silent bug.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationState {
    Idle,
    Initializing,
    LoadingTrainee,
    OpeningForm,
    FillingForm,
    Validating,
    Submitting,
    Verifying,
    Retrying,
    Paused,
    Stopped,
    Complete,
}

use AutomationState::*;

/// The active (in-flight) states — a batch is doing portal work in these.
pub const ACTIVE_STATES: [AutomationState; 8] = [
    Initializing,
    LoadingTrainee,
    OpeningForm,
    FillingForm,
    Validating,
    Submitting,
    Verifying,
    Retrying,
];

pub fn is_active(state: AutomationState) -> bool {
    ACTIVE_STATES.contains(&state)
}

/// Every active state may also retry, advance to the next trainee, pause, stop,
/// or complete — mirrors `fromActive` in AutomationState.ts.
fn from_active(mut next: Vec<AutomationState>) -> Vec<AutomationState> {
    next.extend([Retrying, LoadingTrainee, Paused, Stopped, Complete]);
    next
}

fn allowed(from: AutomationState) -> Vec<AutomationState> {
    match from {
        Idle => vec![Initializing],
        Initializing => from_active(vec![]),
        LoadingTrainee => from_active(vec![OpeningForm]),
        OpeningForm => from_active(vec![FillingForm]),
        FillingForm => from_active(vec![Validating]),
        Validating => from_active(vec![Submitting]),
        Submitting => from_active(vec![Verifying]),
        Verifying => from_active(vec![LoadingTrainee]),
        Retrying => from_active(vec![
            OpeningForm,
            FillingForm,
            Validating,
            Submitting,
            Verifying,
        ]),
        Paused => vec![LoadingTrainee, Stopped, Complete],
        Stopped => vec![Idle],
        Complete => vec![Idle],
    }
}

pub fn can_transition(from: AutomationState, to: AutomationState) -> bool {
    allowed(from).contains(&to)
}

#[derive(Debug)]
pub struct StateMachine {
    current: AutomationState,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self { current: Idle }
    }
}

impl StateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> AutomationState {
        self.current
    }

    pub fn is_active(&self) -> bool {
        is_active(self.current)
    }

    pub fn can_transition_to(&self, next: AutomationState) -> bool {
        can_transition(self.current, next)
    }

    pub fn transition_to(&mut self, next: AutomationState) -> Result<(), String> {
        if !self.can_transition_to(next) {
            return Err(format!(
                "illegal state transition: {:?} -> {:?}",
                self.current, next
            ));
        }
        self.current = next;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.current = Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_for_one_trainee() {
        let mut m = StateMachine::new();
        for step in [
            Initializing,
            LoadingTrainee,
            OpeningForm,
            FillingForm,
            Validating,
            Submitting,
            Verifying,
        ] {
            assert!(m.transition_to(step).is_ok(), "stuck before {step:?}");
        }
        // After verifying, the engine loops to the next trainee.
        assert!(m.transition_to(LoadingTrainee).is_ok());
        assert!(m.transition_to(Complete).is_ok());
    }

    #[test]
    fn rejects_illegal_jumps() {
        let mut m = StateMachine::new();
        assert!(m.transition_to(Submitting).is_err()); // idle can only initialize
        assert!(m.transition_to(Initializing).is_ok());
        assert!(m.transition_to(Verifying).is_err()); // must load/fill/validate first
    }

    #[test]
    fn retry_and_reset() {
        let mut m = StateMachine::new();
        m.transition_to(Initializing).unwrap();
        m.transition_to(LoadingTrainee).unwrap();
        m.transition_to(OpeningForm).unwrap();
        m.transition_to(FillingForm).unwrap();
        m.transition_to(Validating).unwrap();
        m.transition_to(Submitting).unwrap();
        assert!(m.transition_to(Retrying).is_ok());
        assert!(m.transition_to(Submitting).is_ok()); // retry re-submits
        m.reset();
        assert_eq!(m.state(), Idle);
    }

    #[test]
    fn active_flag() {
        assert!(is_active(Submitting));
        assert!(!is_active(Idle));
        assert!(!is_active(Complete));
    }
}
