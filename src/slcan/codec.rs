use super::{SLCAN_ACK, SLCAN_NACK};

use bytes::Buf;
use tokio_util::codec::{Decoder, Encoder};
use tracing::{error, trace};

#[derive(Debug)]
pub struct SlCanCodec;

impl Decoder for SlCanCodec {
    type Item = super::SlCanMessage;
    type Error = super::SlCanError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let eom = src
            .as_ref()
            .iter()
            .position(|b| *b == SLCAN_ACK || *b == SLCAN_NACK);

        if let Some(n) = eom {
            trace!("found data `{:?}`, processing...", src);
            if src[0] == SLCAN_ACK {
                src.advance(1);
                return Ok(Some(Self::Item::Ack));
            } else if src[0] == SLCAN_NACK {
                src.advance(1);
                return Ok(Some(Self::Item::Nack));
            }

            trace!("data was not an ACK or NACK byte, converting to SLCAN frame...");
            let message = src.split_to(n + 1);
            let res = std::str::from_utf8(message.as_ref())
                .map_err(|e| Self::Error::from(e))
                .and_then(|s| super::SlCanFrame::try_create(s))
                .map(|f| Some(Self::Item::Frame(f)));

            if let Err(e) = res {
                error!(
                    "ecountered error decoding slcan frame: data dropped! {:#}",
                    e
                );
                return Ok(None);
            }

            return res;
        }

        Ok(None)
    }
}

impl Encoder<super::SlCanFrame> for SlCanCodec {
    type Error = super::SlCanError;

    fn encode(
        &mut self,
        item: super::SlCanFrame,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        trace!("encoding {:?} to write to serial device", item);

        let item_bytes = item.as_bytes();

        dst.reserve(item_bytes.len() + 1);

        dst.extend_from_slice(item_bytes);
        dst.extend_from_slice(&[SLCAN_ACK]);

        Ok(())
    }
}
