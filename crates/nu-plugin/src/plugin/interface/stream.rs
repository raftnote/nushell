use std::{sync::{mpsc, Mutex, Condvar, Arc, MutexGuard, Weak}, marker::PhantomData, collections::BTreeMap};

use nu_protocol::{Value, ShellError, Span};

use crate::protocol::{StreamMessage, StreamId, StreamData};

#[cfg(test)]
mod tests;

/// Receives messages from a stream read from input by a [`StreamManager`].
///
/// The receiver reads for messages of type `Result<Option<StreamData>, ShellError>` from the
/// channel, which is managed by a [`StreamManager`]. Signalling for end-of-stream is explicit
/// through `Ok(Some)`.
///
/// Failing to receive is an error. When end-of-stream is received, the `receiver` is set to `None`
/// and all further calls to `next()` return `None`.
///
/// The type `T` must implement [`FromShellError`], so that errors in the stream can be represented,
/// and `TryFrom<StreamData>` to convert it to the correct type.
///
/// For each message read, it sends [`StreamMessage::Ack`] to the writer. When dropped,
/// it sends [`StreamMessage::Drop`].
#[derive(Debug)]
pub(crate) struct StreamReader<T, W> where W: WriteStreamMessage {
    id: StreamId,
    receiver: Option<mpsc::Receiver<Result<Option<StreamData>, ShellError>>>,
    writer: W,
    /// Iterator requires the item type to be fixed, so we have to keep it as part of the type,
    /// even though we're actually receiving dynamic data.
    marker: PhantomData<fn() -> T>,
}

impl<T, W> StreamReader<T, W>
where
    T: TryFrom<StreamData, Error = ShellError>,
    W: WriteStreamMessage,
{
    /// Create a new StreamReader from parts
    pub(crate) fn new(
        id: StreamId,
        receiver: mpsc::Receiver<Result<Option<StreamData>, ShellError>>,
        writer: W,
    ) -> StreamReader<T, W> {
        StreamReader {
            id,
            receiver: Some(receiver),
            writer,
            marker: PhantomData,
        }
    }

    /// Receive a message from the channel, or return an error if:
    ///
    /// * the channel couldn't be received from
    /// * an error was sent on the channel
    /// * the message received couldn't be converted to `T`
    pub(crate) fn recv(&mut self) -> Result<Option<T>, ShellError> {
        if let Some(ref rx) = self.receiver {
            // Handle both the receiver error and the on-stream error
            let msg = rx.recv().map_err(|_| ShellError::GenericError {
                error: "Stream ended unexpectedly".into(),
                msg: "connection lost before explicit end of stream".into(),
                span: None,
                help: None,
                inner: vec![],
            })??;

            if let Some(data) = msg {
                // Acknowledge the message
                self.writer.write_stream_message(StreamMessage::Ack(self.id))?;
                // Try to convert it into the correct type
                Ok(Some(data.try_into()?))
            } else {
                // Remove the receiver, so that future recv() calls always return Ok(None)
                self.receiver = None;
                Ok(None)
            }
        } else {
            // Closed already
            Ok(None)
        }
    }
}

impl<T, W> Iterator for StreamReader<T, W>
where
    T: FromShellError + TryFrom<StreamData, Error = ShellError>,
    W: WriteStreamMessage,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        // Converting the error to the value here makes the implementation a lot easier
        self.recv().unwrap_or_else(|err| Some(T::from_shell_error(err)))
    }
}

impl<T, W> Drop for StreamReader<T, W>
where
    W: WriteStreamMessage,
{
    fn drop(&mut self) {
        if let Err(err) = self.writer.write_stream_message(StreamMessage::Drop(self.id)) {
            log::warn!("Failed to send message to drop stream: {err}");
        }
    }
}

/// Values that can contain a `ShellError` to signal an error has occurred.
pub(crate) trait FromShellError {
    fn from_shell_error(err: ShellError) -> Self;
}

// For List streams.
impl FromShellError for Value {
    fn from_shell_error(err: ShellError) -> Self {
        Value::error(err, Span::unknown())
    }
}

// For Raw streams, mostly.
impl<T> FromShellError for Result<T, ShellError> {
    fn from_shell_error(err: ShellError) -> Self {
        Err(err)
    }
}

/// Writes messages to a stream, with flow control.
///
/// The `signal` contained 
#[derive(Debug)]
pub(crate) struct StreamWriter<W: WriteStreamMessage> {
    id: StreamId,
    signal: Arc<StreamWriterSignal>,
    writer: W,
    ended: bool,
}

impl<W> StreamWriter<W> where W: WriteStreamMessage {
    pub(crate) fn new(id: StreamId, signal: Arc<StreamWriterSignal>, writer: W) -> StreamWriter<W> {
        StreamWriter {
            id,
            signal,
            writer,
            ended: false,
        }
    }

    /// Check if the stream was dropped from the other end. Recommended to do this before calling
    /// [`write()`], especially in a loop.
    pub(crate) fn is_dropped(&self) -> Result<bool, ShellError> {
        self.signal.is_dropped()
    }

    /// Write a single piece of data to the stream.
    ///
    /// Error if something failed with the write, or if [`end()`] was already called previously.
    pub(crate) fn write(&mut self, data: impl Into<StreamData>) -> Result<(), ShellError> {
        if !self.ended {
            self.writer.write_stream_message(StreamMessage::Data(self.id, data.into()))?;
            // This implements flow control, so we don't write too many messages:
            self.signal.notify_sent()
        } else {
            Err(ShellError::GenericError {
                error: "Wrote to a stream after it ended".into(),
                msg: format!("tried to write to stream {} after it was already ended", self.id),
                span: None,
                help: Some("this may be a bug in the nu-plugin crate".into()),
                inner: vec![]
            })
        }
    }

    /// Write a full iterator to the stream. Note that this doesn't end the stream, so you should
    /// still call [`end()`].
    ///
    /// If the stream is dropped from the other end, the iterator will not be fully consumed, and
    /// writing will terminate.
    ///
    /// Returns `Ok(true)` if the iterator was fully consumed, or `Ok(false)` if a drop interrupted
    /// the stream from the other side.
    pub(crate) fn write_all<T>(
        &mut self,
        data: impl IntoIterator<Item=T>,
    ) -> Result<bool, ShellError>
    where
        T: Into<StreamData>,
    {
        // Check before starting
        if self.is_dropped()? {
            return Ok(false);
        }

        for item in data {
            // Check again after each item is consumed from the iterator, just in case the iterator
            // takes a while to produce a value
            if self.is_dropped()? {
                return Ok(false);
            }
            self.write(item)?;
        }
        Ok(true)
    }

    /// End the stream. Recommend doing this instead of relying on `Drop` so that you can catch the
    /// error.
    pub(crate) fn end(&mut self) -> Result<(), ShellError> {
        if !self.ended {
            // Set the flag first so we don't double-report in the Drop
            self.ended = true;
            self.writer.write_stream_message(StreamMessage::End(self.id))
        } else {
            Ok(())
        }
    }
}

impl<W> Drop for StreamWriter<W> where W: WriteStreamMessage {
    fn drop(&mut self) {
        // Make sure we ended the stream
        if let Err(err) = self.end() {
            log::warn!("Error while ending stream in Drop for StreamWriter: {err}");
        }
    }
}

/// Stores stream state for a writer, and can be blocked on to wait for messages to be acknowledged.
/// A key part of managing stream lifecycle and flow control.
#[derive(Debug)]
pub(crate) struct StreamWriterSignal {
    mutex: Mutex<StreamWriterSignalState>,
    change_cond: Condvar,
}

#[derive(Debug)]
pub(crate) struct StreamWriterSignalState {
    /// Stream has been dropped and consumer is no longer interested in any messages.
    dropped: bool,
    /// Number of messages that have been sent without acknowledgement.
    unacknowledged: i32,
    /// Max number of messages to send before waiting for acknowledgement.
    high_pressure_mark: i32,
}

impl StreamWriterSignal {
    /// Create a new signal.
    ///
    /// If `notify_sent()` is called more than `high_pressure_mark` times, it will wait until
    /// `notify_acknowledge()` is called by another thread enough times to bring the number of
    /// unacknowledged sent messages below that threshold.
    pub fn new(high_pressure_mark: i32) -> StreamWriterSignal {
        assert!(high_pressure_mark > 0);

        StreamWriterSignal {
            mutex: Mutex::new(StreamWriterSignalState {
                dropped: false,
                unacknowledged: 0,
                high_pressure_mark
            }),
            change_cond: Condvar::new()
        }
    }

    fn lock(&self) -> Result<MutexGuard<StreamWriterSignalState>, ShellError> {
        self.mutex.lock().map_err(|_| ShellError::NushellFailed {
            msg: "StreamWriterSignal mutex poisoned due to panic".into()
        })
    }

    /// True if the stream was dropped and the consumer is no longer interested in it. Indicates
    /// that no more messages should be sent, other than `End`.
    pub fn is_dropped(&self) -> Result<bool, ShellError> {
        Ok(self.lock()?.dropped)
    }

    /// Notify the writers that the stream has been dropped, so they can stop writing.
    pub fn set_dropped(&self) -> Result<(), ShellError> {
        let mut state = self.lock()?;
        state.dropped = true;
        // Unblock the writers so they can terminate
        self.change_cond.notify_all();
        Ok(())
    }

    /// Track that a message has been sent, and wait for the manager to receive acknowledgements
    /// if too many messages have been sent.
    pub fn notify_sent(&self) -> Result<(), ShellError> {
        let mut state = self.lock()?;
        state.unacknowledged = state.unacknowledged.checked_add(1)
            .ok_or_else(|| ShellError::NushellFailed {
                msg: "Overflow in counter: too many unacknowledged messages".into(),
            })?;

        // Wait if too many messages have been sent
        while !state.dropped && state.unacknowledged >= state.high_pressure_mark {
            state = self.change_cond.wait(state).map_err(|_| ShellError::NushellFailed {
                msg: "StreamWriterSignal mutex poisoned due to panic".into()
            })?;
        }
        Ok(())
    }

    /// Notify the writers that a message has been acknowledged, so they can continue to write
    /// if they were waiting.
    pub fn notify_acknowledged(&self) -> Result<(), ShellError> {
        let mut state = self.lock()?;
        state.unacknowledged = state.unacknowledged.checked_sub(1)
            .ok_or_else(|| ShellError::NushellFailed {
                msg: "Underflow in counter: too many message acknowledgements".into()
            })?;
        // Unblock the writer
        self.change_cond.notify_one();
        Ok(())
    }
}

/// A sink for a [`StreamMessage`]
pub(crate) trait WriteStreamMessage {
    fn write_stream_message(&mut self, msg: StreamMessage) -> Result<(), ShellError>;
}

#[derive(Debug, Default)]
struct StreamManagerState {
    reading_streams: BTreeMap<StreamId, mpsc::Sender<Result<Option<StreamData>, ShellError>>>,
    writing_streams: BTreeMap<StreamId, Weak<StreamWriterSignal>>,
}

impl StreamManagerState {
    fn lock(
        state: &Mutex<StreamManagerState>,
    ) -> Result<MutexGuard<StreamManagerState>, ShellError> {
        state.lock().map_err(|_| ShellError::NushellFailed {
            msg: "StreamManagerState mutex poisoned due to a panic".into(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct StreamManager {
    state: Arc<Mutex<StreamManagerState>>,
}

impl StreamManager {
    fn lock(&self) -> Result<MutexGuard<StreamManagerState>, ShellError> {
        StreamManagerState::lock(&self.state)
    }

    pub(crate) fn new() -> StreamManager {
        StreamManager {
            state: Default::default(),
        }
    }

    pub(crate) fn get_handle(&self) -> StreamManagerHandle {
        StreamManagerHandle {
            state: Arc::downgrade(&self.state)
        }
    }

    pub(crate) fn handle_message(&self, message: StreamMessage) -> Result<(), ShellError> {
        let mut state = self.lock()?;
        match message {
            StreamMessage::Data(id, data) => {
                if let Some(sender) = state.reading_streams.get(&id) {
                    // We should ignore the error on send. This just means the reader has dropped,
                    // but it will have sent a Drop message to the other side, and we will receive
                    // an End message at which point we can remove the channel.
                    let _ = sender.send(Ok(Some(data)));
                    Ok(())
                } else {
                    Err(ShellError::PluginFailedToDecode {
                        msg: format!("received Data for unknown stream {id}")
                    })
                }
            }
            StreamMessage::End(id) => {
                if let Some(sender) = state.reading_streams.remove(&id) {
                    // We should ignore the error on the send, because the reader might have dropped
                    // already
                    let _ = sender.send(Ok(None));
                    Ok(())
                } else {
                    Err(ShellError::PluginFailedToDecode {
                        msg: format!("received End for unknown stream {id}")
                    })
                }
            }
            StreamMessage::Drop(id) => {
                if let Some(signal) = state.writing_streams.remove(&id) {
                    if let Some(signal) = signal.upgrade() {
                        // This will wake blocked writers so they can stop writing, so it's ok
                        signal.set_dropped()?;
                    }
                }
                // It's possible that the stream has already finished writing and we don't have it
                // anymore, so we fall through to Ok
                Ok(())
            }
            StreamMessage::Ack(id) => {
                if let Some(signal) = state.writing_streams.get(&id) {
                    if let Some(signal) = signal.upgrade() {
                        // This will wake up a blocked writer
                        signal.notify_acknowledged()?;
                    } else {
                        // We know it doesn't exist, so might as well remove it
                        state.writing_streams.remove(&id);
                    }
                }
                // It's possible that the stream has already finished writing and we don't have it
                // anymore, so we fall through to Ok
                Ok(())
            },
        }
    }

    // If the `StreamManager` is dropped, we should let all of the stream writers know that they
    // won't be able to write anymore. We don't need to do anything about the readers though
    // because they'll know when the `Sender` is dropped automatically
    fn drop_all_writers(&self) -> Result<(), ShellError> {
        let mut state = self.lock()?;
        let writers = std::mem::replace(&mut state.writing_streams, BTreeMap::new());
        for (_, signal) in writers {
            if let Some(signal) = signal.upgrade() {
                // more important that we send to all than handling an error
                let _ = signal.set_dropped();
            }
        }
        Ok(())
    }
}

impl Drop for StreamManager {
    fn drop(&mut self) {
        if let Err(err) = self.drop_all_writers() {
            log::warn!("error during Drop for StreamManager: {}", err)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StreamManagerHandle {
    state: Weak<Mutex<StreamManagerState>>,
}

impl StreamManagerHandle {
    fn with_lock<T, F>(&self, f: F) -> Result<T, ShellError>
    where
        F: FnOnce(MutexGuard<StreamManagerState>) -> Result<T, ShellError>
    {
        let upgraded = self.state.upgrade().ok_or_else(|| ShellError::NushellFailed {
            msg: "StreamManager is no longer alive".into(),
        })?;
        let guard = upgraded.lock().map_err(|_| ShellError::NushellFailed {
            msg: "StreamManagerState mutex poisoned due to a panic".into(),
        })?;
        f(guard)
    }

    pub(crate) fn read_stream<T, W>(
        &self,
        id: StreamId,
        writer: W,
    ) -> Result<StreamReader<T, W>, ShellError>
    where
        T: TryFrom<StreamData, Error = ShellError>,
        W: WriteStreamMessage,
    {
        let (tx, rx) = mpsc::channel();
        self.with_lock(|mut state| {
            // Must be exclusive
            if !state.reading_streams.contains_key(&id) {
                state.reading_streams.insert(id, tx);
                Ok(())
            } else {
                Err(ShellError::GenericError {
                    error: format!("Failed to acquire reader for stream {id}"),
                    msg: "tried to get a reader for a stream that's already being read".into(),
                    span: None,
                    help: Some("this may be a bug in the nu-plugin crate".into()),
                    inner: vec![]
                })
            }
        })?;
        Ok(StreamReader::new(id, rx, writer))
    }

    pub(crate) fn write_stream<W>(
        &self,
        id: StreamId,
        writer: W,
        high_pressure_mark: i32
    ) -> Result<StreamWriter<W>, ShellError>
    where
        W: WriteStreamMessage,
    {
        let signal = Arc::new(StreamWriterSignal::new(high_pressure_mark));
        self.with_lock(|mut state| {
            // Remove dead writing streams
            state.writing_streams.retain(|_, signal| signal.strong_count() > 0);
            // Must be exclusive
            if !state.writing_streams.contains_key(&id) {
                state.writing_streams.insert(id, Arc::downgrade(&signal));
                Ok(())
            } else {
                Err(ShellError::GenericError {
                    error: format!("Failed to acquire writer for stream {id}"),
                    msg: "tried to get a writer for a stream that's already being written".into(),
                    span: None,
                    help: Some("this may be a bug in the nu-plugin crate".into()),
                    inner: vec![]
                })
            }
        })?;
        Ok(StreamWriter::new(id, signal, writer))
    }
}
