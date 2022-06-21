//! # Framed Serial
//!
//! The goal of `framed-serial` is to offer some generic helpers for implementing a buffered,
//! asynchronous serial port writer. It expects devices to support this simple pattern:
//!
//! ```
//! host: write ───► device: do work ──► device: write result ──────┐
//! ▲                                                               │
//! │                                                               │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! It then creates a [`Framed`] sink/source, which is split into seperate read/write tasks, and
//! provides some interfaces for sending messages to these tasks.
//!
//! Consumers of the library can use the multiple producer, single consumer, [`mpsc::Sender`] to
//! ship messages to the device, and can then choose to await the [`oneshot`] response: if the
//! response receiver is dropped, the read task will discard the corresponding device result.
//!
//! The read task may also receive unprompted (non-response) data frames from the serial device.
//! When this occurs, consumers can use the [`broadcast::Receiver`] to stream a copy of this data.
//!
//! NOTE: One pitfall to this method is that, should the device or host drop some result data, the
//! write/response pipes could become mis-aligned.
//!
//! [`Framed`]: tokio_util::codec::Framed
//! [`mpsc::Sender]: tokio::sync::mpsc::Sender
//! [`oneshot`]: tokio::sync::oneshot
//! [`broadcast::Receiver`]: broadcast::Receiver

use std::{borrow, fmt};

use futures::sink::SinkExt;
use futures::stream::{self, StreamExt};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::{io, task};
use tokio_serial::SerialPortBuilderExt;
use tokio_stream::wrappers;
use tokio_util::codec;
use tracing::{trace, warn};

/// [`FramedSerial`] is a helpful way to ship your device around:
///
/// It holds only the channels for communicating with the device, so it can be trivially cloned or
/// shared with other tasks and threads. Several helper functions have be implemented to make it
/// more straightforward to use.
///
/// Creating a framed sender is done by detecting, or opening, a serial device at a particular path,
/// and then using a [`codec`] to build the [`Framed`] serial stream. See `example/slcan.rs` for
/// and example.
///
/// [`Framed`]: tokio_util::codec::Framed
pub struct FramedSerial<F, R> {
    /// Channel for outgoing messages
    sender: mpsc::Sender<F>,
    /// Broadcast channel stored to create new receivers
    broadcast: broadcast::Sender<F>,
    /// Channel to pass result senders to read thread
    results: mpsc::Sender<oneshot::Sender<R>>,
}

impl<F, R> FramedSerial<F, R>
where
    F: Send + Clone + fmt::Debug + TryFrom<R> + 'static,
    R: Clone + fmt::Debug + Send + 'static,
{
    /// Tries to create a new [`FramedSerial`] from a port name and baudrate
    ///
    /// Attempts to open the device at the passed port name using [`tokio_serial`].
    /// If succesful, the device is then decomposed into a stream and sync for message passing.
    pub async fn try_from_port<'a, C, E>(
        name: impl Into<borrow::Cow<'a, str>> + fmt::Debug,
        baudrate: u32,
        codec: C,
    ) -> Result<Self, Error>
    where
        C: codec::Encoder<F> + codec::Decoder<Item = R, Error = E> + Send + 'static,
        E: From<std::io::Error> + fmt::Debug + 'static,
        <C as codec::Encoder<F>>::Error: fmt::Debug,
    {
        tracing::debug!("attempting to open {:?} as an async serial port", name);

        let serial_stream = tokio_serial::new(name, baudrate)
            .open_native_async()
            .map_err(Error::HardwareError)?;

        Ok(Self::new(serial_stream, codec).await)
    }

    /// Tries to detect a usb serial port with the given features, and then defers to [`FramedSerial::try_from_port`]
    ///
    /// This method is only available with feature-flag `util`
    #[cfg(feature = "util")]
    pub async fn try_from_usb_port<C, E>(
        vid: u16,
        pid: u16,
        interface: Option<u8>,
        baudrate: u32,
        codec: C,
    ) -> Result<Self, Error>
    where
        C: codec::Encoder<F> + codec::Decoder<Item = R, Error = E> + Send + 'static,
        E: From<std::io::Error> + fmt::Debug + 'static,
        <C as codec::Encoder<F>>::Error: fmt::Debug,
    {
        let name = util::try_detect_usb_port(vid, pid, interface).map_err(Error::DetectError)?;

        Self::try_from_port(name, baudrate, codec).await
    }

    /// Creates a new [`FramedSerial`] device, using an open [`SerialStream`] and a codec.
    ///
    /// Creates the internal [`Framed`] interface using the provided inputs, and then splits
    /// the device into a sink and stream that are consumed by the devices send/receive tasks.
    ///
    /// [`Framed`]: tokio_util::codec::Framed
    /// [`SerialStream`]: tokio_serial::SerialStream
    pub async fn new<P, C, E>(port: P, codec: C) -> Self
    where
        P: io::AsyncRead + io::AsyncWrite + Send + 'static,
        C: codec::Encoder<F> + codec::Decoder<Item = R, Error = E> + Send + 'static,
        E: From<std::io::Error> + fmt::Debug + 'static,
        <C as codec::Encoder<F>>::Error: fmt::Debug,
    {
        // Create the framed serial sink/stream
        let (sink, stream) = codec::Framed::new(port, codec).split();

        // Create the sub-channels for thread communication
        let (sender, sender_rx) = mpsc::channel(100);
        let (results, results_rx) = mpsc::channel(100);
        let (broadcast, _) = broadcast::channel(100);

        // Start the magic threads
        let frames = broadcast.clone();

        task::spawn(Self::stream(stream, results_rx, frames));
        task::spawn(Self::sink(sink, sender_rx));

        Self {
            sender,
            broadcast,
            results,
        }
    }

    /// Forwards the outgoing message stream
    async fn sink<P, C, E>(
        mut sink: stream::SplitSink<codec::Framed<P, C>, F>,
        mut sender_rx: mpsc::Receiver<F>,
    ) where
        P: io::AsyncRead + io::AsyncWrite,
        C: codec::Encoder<F> + codec::Decoder<Item = R, Error = E>,
        E: From<std::io::Error> + fmt::Debug,
        <C as codec::Encoder<F>>::Error: fmt::Debug,
    {
        while let Some(item) = sender_rx.recv().await {
            trace!("writing message `{:?}` to serial device stream", item);

            // Todo: consider using 'feed' and 'flush' ?
            if let Err(e) = sink.send(item).await {
                tracing::error!("failed to feed item into serial device sink: {:?}", e);
                break;
            }
        }
    }

    /// Parses the incoming message stream
    ///
    /// If the message can be converted into a normal frame, it is broadcast to the data stream.
    /// Otherwise, the message is considered a response frame, and is only sent over the [`oneshot`]
    /// channel provided during message transmission.
    async fn stream<P, C, E>(
        mut stream: stream::SplitStream<codec::Framed<P, C>>,
        mut results: mpsc::Receiver<oneshot::Sender<R>>,
        frames: broadcast::Sender<F>,
    ) where
        P: io::AsyncRead + io::AsyncWrite,
        C: codec::Encoder<F> + codec::Decoder<Item = R, Error = E>,
        E: From<std::io::Error> + fmt::Debug,
    {
        // Process all incoming frames!
        loop {
            match stream.next().await {
                Some(Ok(result_or_frame)) => {
                    trace!("received item `{:?}` from serial device", result_or_frame);

                    // Send to the broadcast channel if this is a data frame
                    if let Ok(frame) = F::try_from(result_or_frame.clone()) {
                        if let Err(e) = frames.send(frame) {
                            warn!("dropped serial frame: {:#}", e)
                        }

                        continue;
                    }

                    // Send the result to the waiting receiver, otherwise
                    match results.try_recv() {
                        Ok(tx) => {
                            if let Err(e) = tx.send(result_or_frame) {
                                trace!("dropped serial response: {:?}", e);
                            }
                        }
                        Err(_) => {
                            warn!(
                                "no response channel was pending, dropped: {:?}",
                                result_or_frame
                            )
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::error!(
                        "encountered hardware error while reading from the serial device: {:?}",
                        e
                    );
                    return;
                }
                None => {
                    // On windows systems, read timeouts will cause a recoverable serial error to occur:
                    // We will catch and return on 'real' serial errors, but loop on 'None'
                    if cfg!(windows) {
                        trace!("encountered probable port-read timeout error, continuing...");
                        continue;
                    }

                    tracing::error!("device stream returned node; read task will exit");
                    return;
                }
            }
        }
    }

    /// Sends a message the serial-device input stream
    ///
    /// Any type that supports `TryInto<F>` can be transmitted to the device. This allows for easy
    /// attachment of different protocols (websocket, REST, etc).
    ///
    /// # Arguments
    /// * `message` - The data convert and then send to the device
    ///
    /// # Returns
    /// Provides a oneshot receiver that can be used to wait on the result of this frame.
    pub async fn send<T>(&self, message: T) -> Result<oneshot::Receiver<R>, Error>
    where
        T: TryInto<F>,
        <T as TryInto<F>>::Error: fmt::Debug + std::error::Error + 'static,
    {
        // Attempts to convert the message into the correct type
        let message = message
            .try_into()
            .map_err(|e| Error::ConversionError(Box::new(e)))?;

        let (tx, rx) = oneshot::channel();

        // If we've stopped receiving results from the hardware, this channel can fill up and block
        // our message transmission!
        if let Err(e) = self.results.send(tx).await {
            tracing::error!("failed to send oneshot receiver to reading thread: {:#}", e);
            return Err(Error::SendError);
        }

        // If writes have backed up on the hardware, this channel will fill and block our message
        // transmission!
        self.sender
            .send(message)
            .await
            .map(|_| rx)
            .map_err(|_| Error::SendError)
    }

    /// Returns a new [`Stream`] wrapper for the devices [`Receiver`]
    ///
    /// Channel messages will be additionally mapped from the device's codec type to any type that
    /// implements `From<F>`.
    ///
    /// [`Stream`]: futures::stream::Stream
    /// [`Receiver`]: tokio::sync::broadcast::Receiver
    pub fn subscribe<T>(&self) -> std::pin::Pin<Box<dyn tokio_stream::Stream<Item = T> + Send>>
    where
        T: From<F>,
    {
        let stream = wrappers::BroadcastStream::from(self.broadcast.subscribe()).filter_map(
            |frame| async move {
                if let Ok(f) = frame {
                    Some(T::from(f))
                } else {
                    None
                }
            },
        );

        Box::pin(stream)
    }
}

/// Errors that may be returned by [`FramedSerial`] commands
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[cfg(feature = "util")]
    #[error("failed to detect the requested serialport")]
    DetectError(#[from] serialport::Error),

    #[error("hardware operation returned an error")]
    HardwareError(#[from] tokio_serial::Error),

    #[error("failed to send a message to the device channel")]
    SendError,

    #[error("failed to convert the message into the expected type")]
    ConversionError(#[from] Box<dyn std::error::Error>),
}

/// Provides a sample codec for SLCAN messages
///
/// # SLCAN Format
/// This format is based on the format described in linux's `slcan.c`:
///
/// The SLCAN ASCII representation of these different frame types is:
/// <type> <id> <dlc> <data>
///
/// Extended frames (29 bit) are defined by capital characters in the type.
/// RTR frames are defined as 'r' types - normal frames have 't' type:
/// t => 11 bit data frame
/// r => 11 bit RTR frame
/// T => 29 bit data frame
/// R => 29 bit RTR frame
///
/// The <id> is 3 (standard) or 8 (extended) bytes in ASCII Hex (base64).
/// The <dlc> is a one byte ASCII number ('0' - '8')
/// The <data> section has at much ASCII Hex bytes as defined by the <dlc>
///
/// ## Examples
///
/// t1230 : can_id 0x123, len 0, no data
/// t4563112233 : can_id 0x456, len 3, data 0x11 0x22 0x33
/// T12ABCDEF2AA55 : extended can_id 0x12ABCDEF, len 2, data 0xAA 0x55
/// r1230 : can_id 0x123, len 0, no data, remote transmission request
#[cfg(feature = "slcan")]
pub mod slcan;

/// Re-export some serial utilities
///
/// Provides optional support for detecting and opening serial devices based on
/// some USB VID, PID, and Interface
#[cfg(feature = "util")]
pub mod util;
