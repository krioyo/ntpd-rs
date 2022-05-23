use crate::{
    packet::NtpLeapIndicator, time_types::PollInterval, NtpDuration, NtpInstant, NtpTimestamp,
};
use tracing::{debug, error, info, instrument, trace};

/// Interface for a clock settable by the ntp implementation.
/// This needs to be a trait as a single system can have multiple clocks
/// which need different implementation for steering and/or now.
pub trait NtpClock {
    type Error: std::error::Error;

    fn now(&self) -> Result<NtpTimestamp, Self::Error>;

    fn set_freq(&self, freq: f64) -> Result<(), Self::Error>;
    fn step_clock(&self, offset: NtpDuration) -> Result<(), Self::Error>;
    fn update_clock(
        &self,
        offset: NtpDuration,
        est_error: NtpDuration,
        max_error: NtpDuration,
        poll_interval: PollInterval,
        leap_status: NtpLeapIndicator,
    ) -> Result<(), Self::Error>;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ClockState {
    StartupBlank,
    // Needed when implementing frequency backups
    #[allow(dead_code)]
    StartupFreq,
    MeasureFreq,
    Spike,
    Sync,
}

/// Controller responsible for actually
/// deciding which adjustments to make based
/// on results from the filtering and
/// combining algorithms.
#[derive(Debug, Copy, Clone)]
pub struct ClockController<C: NtpClock> {
    clock: C,
    state: ClockState,
    last_update_time: NtpInstant,
    preferred_poll_interval: PollInterval,
    poll_interval_counter: i32,
    offset: NtpDuration,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ClockUpdateResult {
    Ignore,
    Step,
    Slew,
    Panic,
}

impl<C: NtpClock> ClockController<C> {
    pub fn new(clock: C) -> Self {
        clock.set_freq(0.).expect("Unable to set clock frequency");
        Self {
            clock,
            state: ClockState::StartupBlank,
            // Setting up the clock counts as an update for
            // the purposes of the math done here
            last_update_time: NtpInstant::now(),
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::ZERO,
        }
    }

    // Preferred ratio between measured offset
    // and measurement jitter
    const POLL_FACTOR: i8 = 4;
    // Threshold for changing desired poll interval
    const POLL_ADJUST: i32 = 30;

    #[instrument(skip(self))]
    pub fn update(
        &mut self,
        offset: NtpDuration,
        jitter: NtpDuration,
        root_delay: NtpDuration,
        root_dispersion: NtpDuration,
        leap_status: NtpLeapIndicator,
        last_peer_update: NtpInstant,
    ) -> ClockUpdateResult {
        // Check that we have a somewhat reasonable result
        if self.offset_too_large(offset) {
            error!("Detected overly large offset");
            return ClockUpdateResult::Panic;
        }

        // Main decision making
        //
        // Combined, this code is responsible for:
        //  - Filtering large but temporary spikes in the measured
        //    offset to our timeservers
        //  - Stepping the clock if a large difference persists long
        //    enough
        //  - Ensuring a proper initial frequency measurement on startup
        //  - Making small (gradual) adjustments to the clock when we
        //    only have a small error
        if offset.abs() > NtpDuration::STEP_THRESHOLD {
            // Large spikes are filtered initialy (to handle weird but temporary network issues)
            // and then handled by stepping if they persist.
            match self.state {
                ClockState::Sync => {
                    info!("Spike detected");
                    self.state = ClockState::Spike;
                    return ClockUpdateResult::Ignore;
                }
                ClockState::MeasureFreq => {
                    if NtpInstant::abs_diff(last_peer_update, self.last_update_time)
                        < NtpDuration::SPIKE_INTERVAL
                    {
                        // Initial frequency measurement needs some time
                        debug!("Frequency measurement not finished yet");
                        return ClockUpdateResult::Ignore;
                    }

                    self.set_freq(offset, last_peer_update);
                    return self.do_step(offset, last_peer_update);
                }
                ClockState::Spike => {
                    if NtpInstant::abs_diff(last_peer_update, self.last_update_time)
                        < NtpDuration::SPIKE_INTERVAL
                    {
                        // Filter out short spikes
                        debug!("Spike continues");
                        return ClockUpdateResult::Ignore;
                    }

                    // Seems that the large difference reflects reality, since
                    // it persisted for a significant amount of time. So step
                    // the clock
                    return self.do_step(offset, last_peer_update);
                }
                ClockState::StartupBlank | ClockState::StartupFreq => {
                    // In fully non-synchronized states, doing the jump
                    // immediately is fine, as we expect the clock to
                    // be off significantly
                    return self.do_step(offset, last_peer_update);
                }
            }
        } else {
            match self.state {
                ClockState::StartupBlank => {
                    // Even though we have a small offset, making a step here
                    // is the easiest way to get into a proper state.
                    //
                    // Using slew might result in us also accidentaly
                    // moving away from the freq=0 initialization done earlier,
                    // ruining the frequency measurement coming after.
                    return self.do_step(offset, last_peer_update);
                }
                ClockState::MeasureFreq => {
                    if NtpInstant::abs_diff(last_peer_update, self.last_update_time)
                        < NtpDuration::SPIKE_INTERVAL
                    {
                        // Initial frequency measurement needs some time
                        debug!("Frequency measurement not finished yet");
                        return ClockUpdateResult::Ignore;
                    }

                    self.set_freq(offset, last_peer_update);
                    self.offset = offset;
                    self.last_update_time = last_peer_update;
                    self.state = ClockState::Sync;
                }
                ClockState::StartupFreq | ClockState::Sync | ClockState::Spike => {
                    // Just make the small adjustment needed, we are good

                    // Since we currently only support the kernel api interface,
                    // we do not need to calculate frequency changes here, the
                    // kernel will do that for us.

                    self.offset = offset;
                    self.last_update_time = last_peer_update;
                    self.state = ClockState::Sync;
                }
            }
        }

        // It is reasonable to panic here, as there is very little we can
        // be expected to do if the clock is not amenable to change
        self.clock
            .update_clock(
                self.offset,
                jitter,
                root_delay / 2 + root_dispersion,
                self.preferred_poll_interval,
                leap_status,
            )
            .expect("Unable to update clock");

        // Adjust whether we would prefer to have a longer or shorter
        // poll interval depending on the amount of jitter
        if self.offset < jitter * Self::POLL_FACTOR {
            self.poll_interval_counter += self.preferred_poll_interval.as_log() as i32;
        } else {
            self.poll_interval_counter -= self.preferred_poll_interval.as_log() as i32;
        }

        trace!(
            counter = debug(self.poll_interval_counter),
            "Poll preference"
        );

        // If our preference becomes strong enough, adjust poll interval
        // and reset. The hysteresis here ensures we aren't constantly flip-flopping
        // between different preferred interval lengths.
        if self.poll_interval_counter > Self::POLL_ADJUST {
            self.poll_interval_counter = 0;
            self.preferred_poll_interval = self.preferred_poll_interval.inc();
            debug!(
                poll_interval = debug(self.preferred_poll_interval),
                "Increased system poll interval"
            );
        }
        if self.poll_interval_counter < -Self::POLL_ADJUST {
            self.poll_interval_counter = 0;
            self.preferred_poll_interval = self.preferred_poll_interval.dec();
            debug!(
                poll_interval = debug(self.preferred_poll_interval),
                "Decreased system poll interval"
            );
        }

        info!(offset = debug(offset), "Slewed clock");
        ClockUpdateResult::Slew
    }

    pub fn preferred_poll_interval(&self) -> PollInterval {
        self.preferred_poll_interval
    }

    fn offset_too_large(&self, offset: NtpDuration) -> bool {
        match self.state {
            // The system might be wildly off on startup
            //  so ignore large steps then
            ClockState::StartupBlank => false,
            ClockState::StartupFreq => false,
            _ => offset.abs() > NtpDuration::PANIC_THRESHOLD,
        }
    }

    fn do_step(&mut self, offset: NtpDuration, last_peer_update: NtpInstant) -> ClockUpdateResult {
        info!(offset = debug(offset), "Stepping clock");
        self.poll_interval_counter = 0;
        self.preferred_poll_interval = PollInterval::MIN;
        // It is reasonable to panic here, as there is very little we can
        // be expected to do if the clock is not amenable to change
        self.clock.step_clock(offset).expect("Unable to step clock");
        self.offset = NtpDuration::ZERO;
        self.last_update_time = last_peer_update;
        self.state = match self.state {
            ClockState::StartupBlank => ClockState::MeasureFreq,
            _ => ClockState::Sync,
        };
        ClockUpdateResult::Step
    }

    fn set_freq(&mut self, offset: NtpDuration, last_peer_update: NtpInstant) {
        info!(
            freq = display(
                offset.to_seconds()
                    / NtpInstant::abs_diff(last_peer_update, self.last_update_time).to_seconds()
            ),
            "Setting initial frequency"
        );
        self.clock
            .set_freq(
                offset.to_seconds()
                    / NtpInstant::abs_diff(last_peer_update, self.last_update_time).to_seconds(),
            )
            .expect("Unable to adjust clock frequency");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use std::time::Duration;

    #[derive(Debug, Clone, Default)]
    struct TestClock {
        last_freq: RefCell<Option<f64>>,
        last_offset: RefCell<Option<NtpDuration>>,
        last_est_error: RefCell<Option<NtpDuration>>,
        last_max_error: RefCell<Option<NtpDuration>>,
        last_poll_interval: RefCell<Option<PollInterval>>,
        last_leap_status: RefCell<Option<NtpLeapIndicator>>,
    }

    impl NtpClock for TestClock {
        type Error = std::io::Error;

        fn now(&self) -> std::result::Result<NtpTimestamp, Self::Error> {
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        }

        fn set_freq(&self, freq: f64) -> Result<(), Self::Error> {
            *self.last_freq.borrow_mut() = Some(freq);
            Ok(())
        }

        fn step_clock(&self, offset: NtpDuration) -> Result<(), Self::Error> {
            *self.last_offset.borrow_mut() = Some(offset);
            Ok(())
        }

        fn update_clock(
            &self,
            offset: NtpDuration,
            est_error: NtpDuration,
            max_error: NtpDuration,
            poll_interval: PollInterval,
            leap_status: NtpLeapIndicator,
        ) -> Result<(), Self::Error> {
            *self.last_offset.borrow_mut() = Some(offset);
            *self.last_est_error.borrow_mut() = Some(est_error);
            *self.last_max_error.borrow_mut() = Some(max_error);
            *self.last_poll_interval.borrow_mut() = Some(poll_interval);
            *self.last_leap_status.borrow_mut() = Some(leap_status);
            Ok(())
        }
    }

    #[test]
    fn test_value_passthrough() {
        let base = NtpInstant::now();

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::Sync,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        let ref_interval = controller.preferred_poll_interval;

        assert_eq!(
            controller.update(
                NtpDuration::from_fixed_int(0),
                NtpDuration::from_fixed_int(50),
                NtpDuration::from_fixed_int(20),
                NtpDuration::from_fixed_int(10),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Slew
        );

        assert_eq!(
            Some(NtpDuration::from_fixed_int(50)),
            *controller.clock.last_est_error.borrow()
        );
        assert_eq!(
            Some(NtpDuration::from_fixed_int(20)),
            *controller.clock.last_max_error.borrow()
        );
        assert_eq!(
            Some(NtpLeapIndicator::NoWarning),
            *controller.clock.last_leap_status.borrow()
        );
        assert_eq!(
            Some(ref_interval),
            *controller.clock.last_poll_interval.borrow()
        );

        controller.preferred_poll_interval = controller.preferred_poll_interval.inc();
        let ref_interval = controller.preferred_poll_interval;

        assert_eq!(
            controller.update(
                NtpDuration::from_fixed_int(0),
                NtpDuration::from_fixed_int(100),
                NtpDuration::from_fixed_int(40),
                NtpDuration::from_fixed_int(60),
                NtpLeapIndicator::Leap59,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Slew
        );

        assert_eq!(
            Some(NtpDuration::from_fixed_int(100)),
            *controller.clock.last_est_error.borrow()
        );
        assert_eq!(
            Some(NtpDuration::from_fixed_int(80)),
            *controller.clock.last_max_error.borrow()
        );
        assert_eq!(
            Some(NtpLeapIndicator::Leap59),
            *controller.clock.last_leap_status.borrow()
        );
        assert_eq!(
            Some(ref_interval),
            *controller.clock.last_poll_interval.borrow()
        );
    }

    #[test]
    fn test_startup_logic() {
        let mut controller = ClockController::new(TestClock::default());
        let base = controller.last_update_time;

        controller.update(
            NtpDuration::from_fixed_int(0),
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(1),
        );

        assert_eq!(controller.state, ClockState::MeasureFreq);
        assert_eq!(
            *controller.clock.last_offset.borrow(),
            Some(NtpDuration::from_fixed_int(0))
        );

        controller.update(
            NtpDuration::from_fixed_int(1 << 32),
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(1801),
        );

        assert_eq!(controller.state, ClockState::Sync);
        assert_eq!(
            *controller.clock.last_offset.borrow(),
            Some(NtpDuration::from_fixed_int(1 << 32))
        );
        assert_eq!(*controller.clock.last_freq.borrow(), Some(1. / 1800.));
    }

    #[test]
    fn test_startup_logic_freq() {
        let base = NtpInstant::now();

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::StartupFreq,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        controller.update(
            NtpDuration::from_fixed_int(0),
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(1),
        );

        assert_eq!(controller.state, ClockState::Sync);
        assert_eq!(
            *controller.clock.last_offset.borrow(),
            Some(NtpDuration::from_fixed_int(0))
        );
    }

    #[test]
    fn test_spike_rejection() {
        let base = NtpInstant::now();

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::Sync,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        controller.update(
            2 * NtpDuration::STEP_THRESHOLD,
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(1),
        );

        assert_eq!(controller.state, ClockState::Spike);
        assert_eq!(*controller.clock.last_offset.borrow(), None);

        controller.update(
            NtpDuration::from_fixed_int(0),
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(2),
        );

        assert_eq!(controller.state, ClockState::Sync);
        assert_eq!(
            *controller.clock.last_offset.borrow(),
            Some(NtpDuration::from_fixed_int(0))
        );
    }

    #[test]
    fn test_spike_acceptance_over_time() {
        let base = NtpInstant::now();

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::Sync,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        controller.update(
            2 * NtpDuration::STEP_THRESHOLD,
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(1),
        );

        assert_eq!(controller.state, ClockState::Spike);
        assert_eq!(*controller.clock.last_offset.borrow(), None);

        controller.update(
            2 * NtpDuration::STEP_THRESHOLD,
            NtpDuration::from_seconds(0.01),
            NtpDuration::from_seconds(0.02),
            NtpDuration::from_seconds(0.03),
            NtpLeapIndicator::NoWarning,
            base + Duration::from_secs(902),
        );

        assert_eq!(controller.state, ClockState::Sync);
        assert_eq!(
            *controller.clock.last_offset.borrow(),
            Some(2 * NtpDuration::STEP_THRESHOLD)
        );
    }

    #[test]
    fn test_excess_detection() {
        let base = NtpInstant::now();

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::Sync,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        assert_eq!(
            controller.update(
                2 * NtpDuration::PANIC_THRESHOLD,
                NtpDuration::from_seconds(0.01),
                NtpDuration::from_seconds(0.02),
                NtpDuration::from_seconds(0.03),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Panic
        );

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::Spike,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        assert_eq!(
            controller.update(
                2 * NtpDuration::PANIC_THRESHOLD,
                NtpDuration::from_seconds(0.01),
                NtpDuration::from_seconds(0.02),
                NtpDuration::from_seconds(0.03),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Panic
        );

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::MeasureFreq,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        assert_eq!(
            controller.update(
                2 * NtpDuration::PANIC_THRESHOLD,
                NtpDuration::from_seconds(0.01),
                NtpDuration::from_seconds(0.02),
                NtpDuration::from_seconds(0.03),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Panic
        );

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::StartupBlank,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        assert_eq!(
            controller.update(
                2 * NtpDuration::PANIC_THRESHOLD,
                NtpDuration::from_seconds(0.01),
                NtpDuration::from_seconds(0.02),
                NtpDuration::from_seconds(0.03),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Step
        );

        let mut controller = ClockController {
            clock: TestClock::default(),
            state: ClockState::StartupFreq,
            last_update_time: base,
            preferred_poll_interval: PollInterval::MIN,
            poll_interval_counter: 0,
            offset: NtpDuration::from_fixed_int(0),
        };

        assert_eq!(
            controller.update(
                2 * NtpDuration::PANIC_THRESHOLD,
                NtpDuration::from_seconds(0.01),
                NtpDuration::from_seconds(0.02),
                NtpDuration::from_seconds(0.03),
                NtpLeapIndicator::NoWarning,
                base + Duration::from_secs(1),
            ),
            ClockUpdateResult::Step
        );
    }
}
