//! Guest logging never writes a registered credential, including fmt writes
//! split across buffers. One guard holds the log record until its final suffix.
use std::{
    io::{self, Write},
    sync::{Arc, Mutex, MutexGuard},
};
use tracing_subscriber::fmt::MakeWriter;
use zuse_providers::credentials::redaction::{Secrets, Text};
struct State {
    filter: Text,
    output: Box<dyn Write + Send>,
}
#[derive(Clone)]
pub struct Writer(Arc<Mutex<State>>);
impl Writer {
    pub fn new(secrets: Arc<Secrets>) -> Self {
        Self::with_output(secrets, Box::new(io::stdout()))
    }
    fn with_output(secrets: Arc<Secrets>, output: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(State {
            filter: Text::new(secrets),
            output,
        })))
    }
}
pub struct Guard<'a>(MutexGuard<'a, State>);
impl Write for Guard<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let clean = self.0.filter.push(bytes);
        self.0.output.write_all(&clean)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let tail = self.0.filter.finish();
        self.0.output.write_all(&tail)?;
        self.0.output.flush()
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
impl<'a> MakeWriter<'a> for Writer {
    type Writer = Guard<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        Guard(
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn log_record_masks_split_and_escaped_credentials() {
        let secrets = Arc::new(Secrets::default());
        secrets.record("selected-agent-key").unwrap();
        secrets.record("multiline\nprivate-key").unwrap();
        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let writer = Writer::with_output(secrets, Box::new(buffer.clone()));
        {
            let mut output = writer.make_writer();
            output.write_all(b"normal selected-").unwrap();
            output.write_all(b"agent-key\n").unwrap();
            output.write_all(br#"multiline\nprivate-key"#).unwrap();
        }
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(!output.contains("selected-agent-key"));
        assert!(!output.contains("multiline\\nprivate-key"));
        assert!(output.contains("normal [redacted]"));
    }
}
