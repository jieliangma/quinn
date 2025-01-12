use crate::connection::spaces::PendingAcks;
use crate::frame::AckFrequency;
use crate::{AckFrequencyConfig, TransportError, VarInt, TIMER_GRANULARITY};
use std::time::Duration;

/// State associated to ACK frequency
pub(super) struct AckFrequencyState {
    //
    // Sending ACK_FREQUENCY frames
    //
    in_flight_ack_frequency_frame: Option<(u64, Duration)>,
    next_outgoing_sequence_number: VarInt,
    pub(super) peer_max_ack_delay: Duration,

    //
    // Receiving ACK_FREQUENCY frames
    //
    last_ack_frequency_frame: Option<u64>,
    pub(super) max_ack_delay: Duration,
}

impl AckFrequencyState {
    pub(super) fn new(default_max_ack_delay: Duration) -> Self {
        Self {
            in_flight_ack_frequency_frame: None,
            next_outgoing_sequence_number: VarInt(0),
            peer_max_ack_delay: default_max_ack_delay,

            last_ack_frequency_frame: None,
            max_ack_delay: default_max_ack_delay,
        }
    }

    /// Returns the `max_ack_delay` that should be requested of the peer when sending an
    /// ACK_FREQUENCY frame
    pub(super) fn candidate_max_ack_delay(&self, config: &AckFrequencyConfig) -> Duration {
        // Use the peer's max_ack_delay if no custom max_ack_delay was provided in the config
        config.max_ack_delay.unwrap_or(self.peer_max_ack_delay)
    }

    /// Returns the `max_ack_delay` for the purposes of calculating the PTO
    ///
    /// This `max_ack_delay` is defined as the maximum of the peer's current `max_ack_delay` and all
    /// in-flight `max_ack_delay`s (i.e. proposed values that haven't been acknowledged yet, but
    /// might be already in use by the peer).
    pub(super) fn max_ack_delay_for_pto(&self) -> Duration {
        // Note: we have at most one in-flight ACK_FREQUENCY frame
        if let Some((_, max_ack_delay)) = self.in_flight_ack_frequency_frame {
            self.peer_max_ack_delay.max(max_ack_delay)
        } else {
            self.peer_max_ack_delay
        }
    }

    /// Returns the next sequence number for an ACK_FREQUENCY frame
    pub(super) fn next_sequence_number(&mut self) -> VarInt {
        assert!(self.next_outgoing_sequence_number <= VarInt::MAX);

        let seq = self.next_outgoing_sequence_number;
        self.next_outgoing_sequence_number.0 += 1;
        seq
    }

    /// Returns true if we should send an ACK_FREQUENCY frame
    pub(super) fn should_send_ack_frequency(&self) -> bool {
        // Currently, we only allow sending a single ACK_FREQUENCY frame. There is no need to send
        // more, because none of the sent values needs to be updated in the course of the connection
        self.next_outgoing_sequence_number.0 == 0
    }

    /// Notifies the [`AckFrequencyState`] that a packet containing an ACK_FREQUENCY frame was sent
    pub(super) fn ack_frequency_sent(&mut self, pn: u64, requested_max_ack_delay: Duration) {
        self.in_flight_ack_frequency_frame = Some((pn, requested_max_ack_delay));
    }

    /// Notifies the [`AckFrequencyState`] that a packet has been ACKed
    pub(super) fn on_acked(&mut self, pn: u64) {
        match self.in_flight_ack_frequency_frame {
            Some((number, requested_max_ack_delay)) if number == pn => {
                self.in_flight_ack_frequency_frame = None;
                self.peer_max_ack_delay = requested_max_ack_delay;
            }
            _ => {}
        }
    }

    /// Notifies the [`AckFrequencyState`] that an ACK_FREQUENCY frame was received
    ///
    /// Updates the endpoint's params according to the payload of the ACK_FREQUENCY frame, or
    /// returns an error in case the requested `max_ack_delay` is invalid.
    ///
    /// Returns `true` if the frame was processed and `false` if it was ignored because of being
    /// stale.
    pub(super) fn ack_frequency_received(
        &mut self,
        frame: &AckFrequency,
        pending_acks: &mut PendingAcks,
    ) -> Result<bool, TransportError> {
        if self
            .last_ack_frequency_frame
            .map_or(false, |highest_sequence_nr| {
                frame.sequence.into_inner() <= highest_sequence_nr
            })
        {
            return Ok(false);
        }

        self.last_ack_frequency_frame = Some(frame.sequence.into_inner());

        // Update max_ack_delay
        let max_ack_delay = Duration::from_micros(frame.request_max_ack_delay.into_inner());
        if max_ack_delay < TIMER_GRANULARITY {
            return Err(TransportError::PROTOCOL_VIOLATION(
                "Requested Max Ack Delay in ACK_FREQUENCY frame is less than min_ack_delay",
            ));
        }
        self.max_ack_delay = max_ack_delay;

        // Update the rest of the params
        pending_acks.set_ack_frequency_params(frame);

        Ok(true)
    }
}
