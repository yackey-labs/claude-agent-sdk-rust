//! Transport layer — abstract `Transport` trait + subprocess CLI implementation.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;

use crate::errors::Result;

pub mod subprocess;

/// Low-level transport for the Claude Code wire protocol.
///
/// Custom transports (e.g. for remote Claude Code servers) implement this
/// trait. The control protocol layer (`crate::query::Query`) is built on top.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Connect / start the underlying transport.
    async fn connect(&mut self) -> Result<()>;

    /// Write a raw line (typically JSON + newline) to the transport.
    async fn write(&self, data: &str) -> Result<()>;

    /// Stream of parsed JSON messages from the transport. Should be called
    /// at most once after `connect()`.
    fn read_messages(&mut self) -> BoxStream<'static, Result<Value>>;

    /// Close the transport and clean up.
    async fn close(&mut self) -> Result<()>;

    /// Whether the transport is currently ready for I/O.
    fn is_ready(&self) -> bool;

    /// Close the input stream (signal EOF to the CLI).
    async fn end_input(&self) -> Result<()>;
}
