use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::{KeyValue, global};

use crate::config::OtelConfig;
use crate::types::InferenceEvent;

/// Initialize OpenTelemetry OTLP exporter.
pub fn init_tracer(config: &OtelConfig) -> Result<(), String> {
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::trace::TracerProvider;

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .build()
        .map_err(|e| format!("failed to create OTLP exporter: {e}"))?;

    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

    global::set_tracer_provider(provider);

    tracing::info!(endpoint = %config.endpoint, "opentelemetry OTLP exporter initialized");
    Ok(())
}

/// Record an inference event as an OpenTelemetry span.
pub fn record_inference_span(event: &InferenceEvent) {
    let tracer = global::tracer("prism");

    tracer.in_span("inference", |cx| {
        let span = cx.span();
        span.set_attribute(KeyValue::new("prism.provider", event.provider.clone()));
        span.set_attribute(KeyValue::new("prism.model", event.model.clone()));
        span.set_attribute(KeyValue::new("prism.status", format!("{:?}", event.status)));
        span.set_attribute(KeyValue::new(
            "prism.input_tokens",
            event.input_tokens as i64,
        ));
        span.set_attribute(KeyValue::new(
            "prism.output_tokens",
            event.output_tokens as i64,
        ));
        span.set_attribute(KeyValue::new(
            "prism.total_tokens",
            event.total_tokens as i64,
        ));
        span.set_attribute(KeyValue::new(
            "prism.estimated_cost_usd",
            event.estimated_cost_usd,
        ));
        span.set_attribute(KeyValue::new("prism.latency_ms", event.latency_ms as i64));

        if let Some(ref trace_id) = event.trace_id {
            span.set_attribute(KeyValue::new("prism.trace_id", trace_id.clone()));
        }
        if let Some(ref task_type) = event.task_type {
            span.set_attribute(KeyValue::new("prism.task_type", task_type.to_string()));
        }
    });
}

/// Shutdown the OpenTelemetry tracer provider.
pub fn shutdown() {
    global::shutdown_tracer_provider();
}
