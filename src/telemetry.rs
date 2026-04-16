//! OpenTelemetry configuration helpers.
//!
//! The Claude Code CLI has built-in OTel instrumentation — spans around model
//! requests, tool calls, hooks, and the agent loop. The SDK doesn't produce
//! telemetry itself; it configures the CLI by passing environment variables.
//!
//! This module provides a fluent builder so you don't have to remember
//! 8+ env var names.
//!
//! # Quick example
//!
//! ```no_run
//! # async fn run() -> claude_agent_sdk::Result<()> {
//! use claude_agent_sdk::{Claude, Telemetry};
//!
//! let reply = Claude::builder()
//!     .telemetry(Telemetry::new("https://api.honeycomb.io")
//!         .service_name("my-agent")
//!         .header("x-honeycomb-team", "your-api-key")
//!         .traces()
//!         .metrics()
//!         .logs())
//!     .ask("What files are in this directory?")
//!     .await?;
//! # Ok(()) }
//! ```

use std::collections::HashMap;

/// Fluent builder for OpenTelemetry configuration.
///
/// Produces a set of environment variables that the Claude Code CLI reads
/// to enable and configure telemetry export.
///
/// # Signals
///
/// Call `.traces()`, `.metrics()`, and/or `.logs()` to enable each signal.
/// By default no signals are enabled — you must opt in.
///
/// Traces require the beta flag (`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`),
/// which `.traces()` sets automatically.
///
/// # What gets traced
///
/// | Span | Description |
/// |---|---|
/// | `claude_code.interaction` | One turn of the agent loop (prompt → response) |
/// | `claude_code.llm_request` | Each Claude API call (model, latency, tokens) |
/// | `claude_code.tool` | Each tool invocation |
/// | `claude_code.tool.blocked_on_user` | Time waiting for permission approval |
/// | `claude_code.tool.execution` | Tool execution time |
/// | `claude_code.hook` | Each hook execution |
///
/// All spans carry a `session.id` attribute for cross-query correlation.
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    endpoint: Option<String>,
    protocol: Option<String>,
    headers: Vec<(String, String)>,
    service_name: Option<String>,
    resource_attrs: Vec<(String, String)>,
    enable_traces: bool,
    enable_metrics: bool,
    enable_logs: bool,
    log_user_prompts: bool,
    log_tool_details: bool,
    log_tool_content: bool,
    metric_export_interval_ms: Option<u64>,
    traces_export_interval_ms: Option<u64>,
    logs_export_interval_ms: Option<u64>,
}

impl Telemetry {
    /// Create a new telemetry config pointing at an OTLP endpoint.
    ///
    /// ```no_run
    /// # use claude_agent_sdk::Telemetry;
    /// let t = Telemetry::new("https://api.honeycomb.io");
    /// let t = Telemetry::new("http://localhost:4318");
    /// ```
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self { endpoint: Some(endpoint.into()), ..Default::default() }
    }

    /// Set the OTLP protocol. Defaults to `http/protobuf` if not set.
    pub fn protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }

    /// Add an OTLP header (e.g. auth tokens).
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Set the service name (default: `"claude-code"`).
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    /// Add an OTel resource attribute (e.g. `service.version`, `deployment.environment`).
    pub fn resource_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource_attrs.push((key.into(), value.into()));
        self
    }

    /// Enable trace export (spans for agent loop, model requests, tool calls).
    /// Automatically enables the traces beta flag.
    pub fn traces(mut self) -> Self {
        self.enable_traces = true;
        self
    }

    /// Enable metrics export (token counters, cost, session counts).
    pub fn metrics(mut self) -> Self {
        self.enable_metrics = true;
        self
    }

    /// Enable log event export (prompts, API requests, tool results).
    pub fn logs(mut self) -> Self {
        self.enable_logs = true;
        self
    }

    /// Enable all three signals (traces + metrics + logs).
    pub fn all(self) -> Self {
        self.traces().metrics().logs()
    }

    /// Include user prompt text in telemetry.
    /// Only enable if your backend is approved for this data.
    pub fn log_user_prompts(mut self) -> Self {
        self.log_user_prompts = true;
        self
    }

    /// Include tool input arguments (file paths, commands) in telemetry.
    pub fn log_tool_details(mut self) -> Self {
        self.log_tool_details = true;
        self
    }

    /// Include full tool input/output bodies (truncated at 60KB) as span events.
    /// Requires traces to be enabled.
    pub fn log_tool_content(mut self) -> Self {
        self.log_tool_content = true;
        self
    }

    /// Set the export interval for all signals (milliseconds).
    /// Default: metrics=60000, traces/logs=5000.
    /// Lower values reduce data loss on crash but increase export overhead.
    pub fn export_interval_ms(mut self, ms: u64) -> Self {
        self.metric_export_interval_ms = Some(ms);
        self.traces_export_interval_ms = Some(ms);
        self.logs_export_interval_ms = Some(ms);
        self
    }

    /// Convert to environment variables for the CLI.
    pub fn to_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("CLAUDE_CODE_ENABLE_TELEMETRY".into(), "1".into());

        if let Some(endpoint) = &self.endpoint {
            env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), endpoint.clone());
        }
        let protocol = self.protocol.as_deref().unwrap_or("http/protobuf");
        env.insert("OTEL_EXPORTER_OTLP_PROTOCOL".into(), protocol.into());

        if !self.headers.is_empty() {
            let h: Vec<String> = self.headers.iter().map(|(k, v)| format!("{k}={v}")).collect();
            env.insert("OTEL_EXPORTER_OTLP_HEADERS".into(), h.join(","));
        }
        if let Some(name) = &self.service_name {
            env.insert("OTEL_SERVICE_NAME".into(), name.clone());
        }
        if !self.resource_attrs.is_empty() {
            let a: Vec<String> = self.resource_attrs.iter().map(|(k, v)| format!("{k}={v}")).collect();
            env.insert("OTEL_RESOURCE_ATTRIBUTES".into(), a.join(","));
        }

        if self.enable_traces {
            env.insert("OTEL_TRACES_EXPORTER".into(), "otlp".into());
            env.insert("CLAUDE_CODE_ENHANCED_TELEMETRY_BETA".into(), "1".into());
        }
        if self.enable_metrics {
            env.insert("OTEL_METRICS_EXPORTER".into(), "otlp".into());
        }
        if self.enable_logs {
            env.insert("OTEL_LOGS_EXPORTER".into(), "otlp".into());
        }

        if self.log_user_prompts {
            env.insert("OTEL_LOG_USER_PROMPTS".into(), "1".into());
        }
        if self.log_tool_details {
            env.insert("OTEL_LOG_TOOL_DETAILS".into(), "1".into());
        }
        if self.log_tool_content {
            env.insert("OTEL_LOG_TOOL_CONTENT".into(), "1".into());
        }

        if let Some(ms) = self.metric_export_interval_ms {
            env.insert("OTEL_METRIC_EXPORT_INTERVAL".into(), ms.to_string());
        }
        if let Some(ms) = self.traces_export_interval_ms {
            env.insert("OTEL_TRACES_EXPORT_INTERVAL".into(), ms.to_string());
        }
        if let Some(ms) = self.logs_export_interval_ms {
            env.insert("OTEL_LOGS_EXPORT_INTERVAL".into(), ms.to_string());
        }

        env
    }
}

// ---------------------------------------------------------------------------
// Preset constructors for popular backends
// ---------------------------------------------------------------------------

impl Telemetry {
    /// Preconfigured for Honeycomb. Pass your API key.
    ///
    /// ```no_run
    /// # use claude_agent_sdk::Telemetry;
    /// let t = Telemetry::honeycomb("your-api-key", "my-agent");
    /// ```
    pub fn honeycomb(api_key: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self::new("https://api.honeycomb.io")
            .header("x-honeycomb-team", api_key)
            .service_name(service_name)
            .all()
    }

    /// Preconfigured for a local OTel collector (e.g. for development).
    ///
    /// ```no_run
    /// # use claude_agent_sdk::Telemetry;
    /// let t = Telemetry::local("my-agent");
    /// ```
    pub fn local(service_name: impl Into<String>) -> Self {
        Self::new("http://localhost:4318")
            .service_name(service_name)
            .all()
            .export_interval_ms(1000)
    }

}
