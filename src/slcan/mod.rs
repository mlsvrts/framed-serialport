use std::borrow::Cow;

pub const SLCAN_ACK: u8 = 0x0D;
pub const SLCAN_NACK: u8 = 0x07;

/// Received [`SlCanMessage`]s can take the form of an `ack`/`nack` response byte
/// or be a CAN data/remote frame
#[derive(Clone, Debug)]
pub enum SlCanMessage {
    /// Received an ACK byte in response to a frame
    Ack,
    /// Received a NACK byte in response to a frame
    Nack,
    /// An SLCAN Frame
    Frame(SlCanFrame),
}

/// A newtype wrapper for SLCAN data frames
#[derive(Clone, Debug)]
pub struct SlCanFrame(String);

impl SlCanFrame {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn try_create<'a, T>(value: T) -> Result<Self, SlCanError>
    where
        T: Into<Cow<'a, str>>,
    {
        let cow = value.into();

        if cow.is_empty() {
            return Err(SlCanError::EmptyMessage);
        }

        match &cow[0..1] {
            "r" | "R" | "t" | "T" | "O" | "C" => Ok(SlCanFrame(cow.into_owned())),
            v => Err(SlCanError::BadSof(v.as_bytes()[0])),
        }
    }

    pub fn open() -> Self {
        Self("O".to_string())
    }

    pub fn close() -> Self {
        Self("C".to_string())
    }

    pub fn baudrate(rate: u32) -> Result<Self, SlCanError> {
        let rate = match rate {
            10000 => "S0",
            20000 => "S1",
            50000 => "S2",
            100000 => "S3",
            125000 => "S4",
            250000 => "S5",
            500000 => "S6",
            800000 => "S7",
            1000000 => "S8",
            baud => return Err(SlCanError::InvalidBaudrate(baud)),
        };

        Ok(Self(rate.to_string()))
    }
}

impl TryFrom<&str> for SlCanFrame {
    type Error = SlCanError;

    fn try_from(other: &str) -> Result<Self, Self::Error> {
        SlCanFrame::try_create(other)
    }
}

impl TryFrom<SlCanMessage> for SlCanFrame {
    type Error = SlCanError;

    fn try_from(other: SlCanMessage) -> Result<Self, Self::Error> {
        if let SlCanMessage::Frame(frame) = other {
            Ok(frame)
        } else {
            Err(SlCanError::NotADataFrame)
        }
    }
}

/// Helper error type for the SlCan codec
#[derive(thiserror::Error, Debug)]
pub enum SlCanError {
    #[error("SLCAN device NACK'd the command")]
    Nack,

    #[error("bad start-of-frame byte in slcan message: `{0}`")]
    BadSof(u8),

    #[error("cannot convert empty message")]
    EmptyMessage,

    #[error("failed to interpret byte-vector as UTF8")]
    Utf8Error(#[from] std::str::Utf8Error),

    #[error("encountered hardware io error")]
    IoError(#[from] std::io::Error),

    #[error("`{0}` is not a valid slcan baudrate")]
    InvalidBaudrate(u32),

    #[error("slcan message was not a data frame")]
    NotADataFrame,
}

/// Implements [`Encoder`] and [`Decoder`] for [`SlCanMessage`]
///
/// [`tokio`] encoders can be used to create framed streams/sinks for asynchronous io resources;
/// this implementation will allow us to connect our stream of incoming can frames to our hardware
/// serial device.
///
/// This codec specifically implements a conversion from 'slcan' byte streams to [`SlCanMessage`]s
///
/// [`Encoder`]: `tokio_util::codec::Encoder`
/// [`Decoder`]: `tokio_util::codec::Decoder`
pub mod codec;
pub use codec::SlCanCodec;
