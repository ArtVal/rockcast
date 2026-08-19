//! Post-TLS device authentication handshake.

use std::{
    sync::atomic::AtomicBool,
    time::Duration,
};

use super::{
    consts::{NS_DEVICEAUTH, RECEIVER_ID, SENDER_ID},
    CastChannel, ChannelError,
};
use super::super::proto::{encode_auth_challenge, CastMessage};

impl CastChannel {
    pub(super) fn authenticate(&self) -> Result<(), ChannelError> {
        let challenge = CastMessage::binary(
            SENDER_ID,
            RECEIVER_ID,
            NS_DEVICEAUTH,
            encode_auth_challenge(),
        );
        self.send(&challenge)?;
        // Wait for the auth reply; anything else goes into the inbox.
        let never = AtomicBool::new(false);
        let _ = self.receive_find(&never, Duration::from_secs(8), |msg| {
            if msg.namespace == NS_DEVICEAUTH {
                Ok(Some(()))
            } else {
                Ok(None)
            }
        });
        Ok(())
    }
}
