use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum State {
    Ahead,
    OnTrack,
    Behind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pace {
    pub state: State,
    pub expected_pct: f64,
    pub delta_pct: f64,
}

/// Compare actual usage against the pace expected for the elapsed part of a window.
/// `reset_after_secs` is the time left in the window, so elapsed = window - remaining.
pub fn assess(window_secs: u64, reset_after_secs: u64, used_pct: f64, trigger: f64) -> Option<Pace> {
    if window_secs == 0 || reset_after_secs == 0 || reset_after_secs > window_secs {
        return None;
    }
    assess_elapsed(window_secs - reset_after_secs, window_secs, used_pct, trigger)
}

/// Same idea for windows whose start we only know from local logs (or calendar).
pub fn assess_elapsed(elapsed_secs: u64, window_secs: u64, used_pct: f64, trigger: f64) -> Option<Pace> {
    if window_secs == 0 || elapsed_secs == 0 || elapsed_secs > window_secs {
        return None;
    }
    let expected = (elapsed_secs as f64 / window_secs as f64) * 100.0;
    let delta = used_pct - expected;
    let state = if delta <= -trigger {
        State::Ahead
    } else if delta >= trigger {
        State::Behind
    } else {
        State::OnTrack
    };
    Some(Pace {
        state,
        expected_pct: expected,
        delta_pct: delta,
    })
}

pub fn label(p: &Pace) -> String {
    match p.state {
        State::Ahead => format!("pace ahead {:.0}%", p.delta_pct.abs()),
        State::Behind => format!("pace behind {:.0}%", p.delta_pct),
        State::OnTrack => "pace on track".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfway_through_window_on_track() {
        let p = assess(18000, 9000, 50.0, 10.0).unwrap();
        assert_eq!(p.state, State::OnTrack);
        assert_eq!(p.expected_pct, 50.0);
    }

    #[test]
    fn ahead_of_schedule_when_usage_lags_elapsed_time() {
        let p = assess(604800, 302400, 20.0, 10.0).unwrap();
        assert_eq!(p.state, State::Ahead);
        assert_eq!(p.delta_pct, -30.0);
        assert_eq!(label(&p), "pace ahead 30%");
    }

    #[test]
    fn behind_of_schedule_when_usage_runs_hot() {
        let p = assess(18000, 9000, 75.0, 10.0).unwrap();
        assert_eq!(p.state, State::Behind);
        assert_eq!(label(&p), "pace behind 25%");
    }

    #[test]
    fn no_pace_without_a_running_window() {
        assert!(assess(0, 0, 10.0, 10.0).is_none());
        assert!(assess(18000, 0, 10.0, 10.0).is_none());
        assert!(assess(18000, 20000, 10.0, 10.0).is_none());
    }
}
