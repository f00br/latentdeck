//! Deterministic transport scheduling above Worker Protocol 1.

use latentdeck_control::{
    Command, DecodeCycleAck, ResetReason, SlotDecodeCycle, SlotLoaded, SlotReset, SlotResetAck,
};
use thiserror::Error;

/// One validated slot's decode/reset cursor.
///
/// The scheduler owns no clock and performs no latent math. It only enforces
/// the causal ordering promised by Worker Protocol 1.
#[derive(Debug, Clone)]
pub struct PlaybackSchedule {
    slot: SlotLoaded,
    generation: u64,
    next_cycle_index: u64,
    end_of_stream: bool,
    pending_reset_generation: Option<u64>,
}

impl PlaybackSchedule {
    /// Start at the first cycle of a freshly loaded slot.
    ///
    /// # Errors
    ///
    /// Returns an error when Core attempts to start generation zero.
    pub fn new(slot: SlotLoaded, generation: u64) -> Result<Self, ScheduleError> {
        if generation == 0 {
            return Err(ScheduleError::GenerationZero);
        }
        Ok(Self {
            slot,
            generation,
            next_cycle_index: 0,
            end_of_stream: false,
            pending_reset_generation: None,
        })
    }

    /// Current stream generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Cycle expected by the next decode acknowledgement.
    #[must_use]
    pub const fn next_cycle_index(&self) -> u64 {
        self.next_cycle_index
    }

    /// Whether the current causal pass reached its final cycle.
    #[must_use]
    pub const fn end_of_stream(&self) -> bool {
        self.end_of_stream
    }

    /// Build the only legal next decode command.
    ///
    /// Returns `None` after EOS or while a reset acknowledgement is pending.
    #[must_use]
    pub fn next_decode_command(&self) -> Option<Command> {
        if self.end_of_stream || self.pending_reset_generation.is_some() {
            return None;
        }
        Some(Command::SlotDecodeCycle(SlotDecodeCycle {
            slot_id: self.slot.slot_id.clone(),
            slot_revision: self.slot.slot_revision,
            stream_generation: self.generation,
            cycle_index: self.next_cycle_index,
        }))
    }

    /// Verify and commit one decode acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale slot/generation, an out-of-order cycle,
    /// an unexpected codec cadence, or an incorrect EOS marker.
    pub fn accept_decode(&mut self, ack: &DecodeCycleAck) -> Result<(), ScheduleError> {
        self.ensure_slot(&ack.slot_id, ack.slot_revision)?;
        if ack.stream_generation != self.generation {
            return Err(ScheduleError::StaleGeneration {
                expected: self.generation,
                actual: ack.stream_generation,
            });
        }
        if ack.cycle_index != self.next_cycle_index {
            return Err(ScheduleError::CycleOutOfOrder {
                expected: self.next_cycle_index,
                actual: ack.cycle_index,
            });
        }
        let expected = self.expected_cycle(ack.cycle_index)?;
        if ack.latent_start != expected.latent_start
            || ack.latent_count != expected.latent_count
            || ack.decoded_start_frame != expected.decoded_start
            || ack.decoded_frame_count != expected.decoded_count
        {
            return Err(ScheduleError::CadenceMismatch);
        }
        let expected_eos = ack.cycle_index + 1 == self.slot.timing.cycle_count;
        if ack.end_of_stream != expected_eos {
            return Err(ScheduleError::EndOfStreamMismatch);
        }

        self.next_cycle_index = self
            .next_cycle_index
            .checked_add(1)
            .ok_or(ScheduleError::CycleExhausted)?;
        self.end_of_stream = expected_eos;
        Ok(())
    }

    /// Begin an explicit Loop/Restart/Recovery reset with a strictly newer
    /// stream generation. The active generation changes only after the worker
    /// confirms that both decoder and ring state were cleared.
    ///
    /// # Errors
    ///
    /// Returns an error when another reset is pending or generation space is
    /// exhausted.
    pub fn begin_reset(&mut self, reason: ResetReason) -> Result<Command, ScheduleError> {
        if self.pending_reset_generation.is_some() {
            return Err(ScheduleError::ResetPending);
        }
        let new_generation = self
            .generation
            .checked_add(1)
            .ok_or(ScheduleError::GenerationExhausted)?;
        self.pending_reset_generation = Some(new_generation);
        Ok(Command::SlotReset(SlotReset {
            slot_id: self.slot.slot_id.clone(),
            slot_revision: self.slot.slot_revision,
            new_stream_generation: new_generation,
            reason,
        }))
    }

    /// Verify and commit the reset acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns an error unless the worker confirmed the exact pending
    /// generation, cycle zero, and a cleared ring sequence.
    pub fn accept_reset(&mut self, ack: &SlotResetAck) -> Result<(), ScheduleError> {
        self.ensure_slot(&ack.slot_id, ack.slot_revision)?;
        let expected_generation = self
            .pending_reset_generation
            .ok_or(ScheduleError::ResetNotPending)?;
        if ack.stream_generation != expected_generation {
            return Err(ScheduleError::StaleGeneration {
                expected: expected_generation,
                actual: ack.stream_generation,
            });
        }
        if ack.next_cycle_index != 0 || ack.ring_write_sequence != 0 {
            return Err(ScheduleError::ResetDidNotClearState);
        }
        self.generation = expected_generation;
        self.next_cycle_index = 0;
        self.end_of_stream = false;
        self.pending_reset_generation = None;
        Ok(())
    }

    fn ensure_slot(&self, slot_id: &str, slot_revision: u64) -> Result<(), ScheduleError> {
        if slot_id != self.slot.slot_id || slot_revision != self.slot.slot_revision {
            return Err(ScheduleError::StaleSlot);
        }
        Ok(())
    }

    fn expected_cycle(&self, cycle_index: u64) -> Result<ExpectedCycle, ScheduleError> {
        let timing = &self.slot.timing;
        if cycle_index >= timing.cycle_count {
            return Err(ScheduleError::CycleOutOfRange);
        }
        let pattern = if cycle_index >= timing.initial.first_cycle_index
            && cycle_index
                < timing
                    .initial
                    .first_cycle_index
                    .checked_add(timing.initial.cycle_count)
                    .ok_or(ScheduleError::CadenceMismatch)?
        {
            &timing.initial
        } else if cycle_index >= timing.steady.first_cycle_index
            && cycle_index
                < timing
                    .steady
                    .first_cycle_index
                    .checked_add(timing.steady.cycle_count)
                    .ok_or(ScheduleError::CadenceMismatch)?
        {
            &timing.steady
        } else {
            return Err(ScheduleError::CadenceMismatch);
        };
        let offset = cycle_index
            .checked_sub(pattern.first_cycle_index)
            .ok_or(ScheduleError::CadenceMismatch)?;
        Ok(ExpectedCycle {
            latent_start: pattern
                .latent_base
                .checked_add(
                    offset
                        .checked_mul(u64::from(pattern.latent_stride))
                        .ok_or(ScheduleError::CadenceMismatch)?,
                )
                .ok_or(ScheduleError::CadenceMismatch)?,
            latent_count: pattern.latent_count,
            decoded_start: pattern
                .decoded_base
                .checked_add(
                    offset
                        .checked_mul(u64::from(pattern.decoded_stride))
                        .ok_or(ScheduleError::CadenceMismatch)?,
                )
                .ok_or(ScheduleError::CadenceMismatch)?,
            decoded_count: pattern.decoded_count,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ExpectedCycle {
    latent_start: u64,
    latent_count: u32,
    decoded_start: u64,
    decoded_count: u32,
}

/// A violation of the trusted Core transport schedule.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScheduleError {
    #[error("stream generation must be nonzero")]
    GenerationZero,
    #[error("stream generation is exhausted")]
    GenerationExhausted,
    #[error("decode cycle counter is exhausted")]
    CycleExhausted,
    #[error("a reset acknowledgement is already pending")]
    ResetPending,
    #[error("no reset acknowledgement is pending")]
    ResetNotPending,
    #[error("worker acknowledgement belongs to a stale slot")]
    StaleSlot,
    #[error("worker generation is {actual}, expected {expected}")]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("worker cycle is {actual}, expected {expected}")]
    CycleOutOfOrder { expected: u64, actual: u64 },
    #[error("decode cycle is outside the declared timing")]
    CycleOutOfRange,
    #[error("worker decode cadence does not match slot timing")]
    CadenceMismatch,
    #[error("worker EOS marker does not match slot timing")]
    EndOfStreamMismatch,
    #[error("worker reset did not clear causal/ring state")]
    ResetDidNotClearState,
}

#[cfg(test)]
mod tests {
    use latentdeck_control::{CyclePattern, ProfileRef, TimingDescriptor};

    use super::*;

    #[test]
    fn exact_h3_32_slot_cadence_reaches_107_frames_in_order() {
        let mut schedule = PlaybackSchedule::new(slot_loaded(32, 107, 7), 1).expect("schedule");
        for cycle in 0..7 {
            let command = schedule.next_decode_command().expect("decode command");
            let Command::SlotDecodeCycle(command) = command else {
                panic!("decode command required");
            };
            assert_eq!(command.cycle_index, cycle);
            let (latent_start, latent_count, decoded_start, decoded_count) = if cycle == 0 {
                (0, 2, 0, 5)
            } else {
                (2 + (cycle - 1) * 5, 5, 5 + (cycle - 1) * 17, 17)
            };
            schedule
                .accept_decode(&DecodeCycleAck {
                    slot_id: "player-a".to_owned(),
                    slot_revision: 9,
                    stream_generation: 1,
                    cycle_index: cycle,
                    latent_start,
                    latent_count,
                    decoded_start_frame: decoded_start,
                    decoded_frame_count: decoded_count,
                    ring_first_sequence: decoded_start,
                    ring_last_sequence_exclusive: decoded_start + u64::from(decoded_count),
                    end_of_stream: cycle == 6,
                })
                .expect("matching acknowledgement");
        }
        assert!(schedule.end_of_stream());
        assert!(schedule.next_decode_command().is_none());
    }

    #[test]
    fn restart_changes_generation_only_after_a_cleared_reset_ack() {
        let mut schedule = PlaybackSchedule::new(slot_loaded(32, 107, 7), 4).expect("schedule");
        let command = schedule.begin_reset(ResetReason::Restart).expect("reset");
        let Command::SlotReset(command) = command else {
            panic!("reset command required");
        };
        assert_eq!(command.new_stream_generation, 5);
        assert_eq!(schedule.generation(), 4);
        assert!(schedule.next_decode_command().is_none());

        schedule
            .accept_reset(&SlotResetAck {
                slot_id: "player-a".to_owned(),
                slot_revision: 9,
                stream_generation: 5,
                next_cycle_index: 0,
                ring_write_sequence: 0,
            })
            .expect("cleared reset");
        assert_eq!(schedule.generation(), 5);
        assert_eq!(schedule.next_cycle_index(), 0);
        assert!(schedule.next_decode_command().is_some());
    }

    #[test]
    fn wrong_eos_and_uncleared_reset_are_rejected() {
        let mut schedule = PlaybackSchedule::new(slot_loaded(32, 107, 7), 1).expect("schedule");
        let error = schedule
            .accept_decode(&DecodeCycleAck {
                slot_id: "player-a".to_owned(),
                slot_revision: 9,
                stream_generation: 1,
                cycle_index: 0,
                latent_start: 0,
                latent_count: 2,
                decoded_start_frame: 0,
                decoded_frame_count: 5,
                ring_first_sequence: 0,
                ring_last_sequence_exclusive: 5,
                end_of_stream: true,
            })
            .expect_err("early EOS");
        assert_eq!(error, ScheduleError::EndOfStreamMismatch);

        schedule.begin_reset(ResetReason::Recovery).expect("reset");
        let error = schedule
            .accept_reset(&SlotResetAck {
                slot_id: "player-a".to_owned(),
                slot_revision: 9,
                stream_generation: 2,
                next_cycle_index: 0,
                ring_write_sequence: 1,
            })
            .expect_err("uncleared ring");
        assert_eq!(error, ScheduleError::ResetDidNotClearState);
    }

    fn slot_loaded(latent_slots: u64, decoded_frames: u64, cycle_count: u64) -> SlotLoaded {
        SlotLoaded {
            slot_id: "player-a".to_owned(),
            slot_revision: 9,
            width: 800,
            height: 448,
            profile: ProfileRef {
                codec_family: "minimax_h3".to_owned(),
                profile: "h3_av_latent".to_owned(),
                profile_version: "0.1.0".to_owned(),
            },
            timing: TimingDescriptor {
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                latent_slot_count: latent_slots,
                decoded_frame_count: decoded_frames,
                cycle_count,
                initial: CyclePattern {
                    first_cycle_index: 0,
                    cycle_count: 1,
                    latent_base: 0,
                    latent_stride: 0,
                    latent_count: 2,
                    decoded_base: 0,
                    decoded_stride: 0,
                    decoded_count: 5,
                },
                steady: CyclePattern {
                    first_cycle_index: 1,
                    cycle_count: cycle_count - 1,
                    latent_base: 2,
                    latent_stride: 5,
                    latent_count: 5,
                    decoded_base: 5,
                    decoded_stride: 17,
                    decoded_count: 17,
                },
                reset_required_on_wrap: true,
                arbitrary_seek: false,
                max_frames_per_cycle: 17,
            },
        }
    }
}
