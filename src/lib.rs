use axum::{
    routing::{get, post},
    Router,
};
use axum_prometheus::PrometheusMetricLayer;
use axum_tracing_opentelemetry::middleware::OtelAxumLayer;
use llm::Model;
use serde::de;
use serde::Serialize;
use serde::{Deserialize, Deserializer};
use std::convert::Infallible;
use std::marker::PhantomData;
use std::{fmt, sync::Arc};
pub mod defaults;
use defaults::*;
pub mod config;
use config::Config;

use crate::routes::{
    chat::chat_completion,
    completions::{compat_completions, completions, completions_stream},
    embeddings::embeddings,
    models::get_models,
};
pub mod routes;

pub const N_SUPPORTED_MODELS: usize = 1;

#[derive(Serialize, Deserialize, Clone)]
pub struct ModelList {
    pub data: [String; N_SUPPORTED_MODELS],
}

pub async fn run_webserver(config: Config) {
    let model_architecture = config.model_architecture;
    let model_path = config.model_path.clone();
    let tokenizer_source = config.to_tokenizer_source();
    let model_params = config.extract_model_params();

    // we init prometheus metrics
    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();

    let now = std::time::Instant::now();

    let model: Arc<dyn Model> = Arc::from(
        llm::load_dynamic(
            Some(model_architecture),
            &model_path,
            tokenizer_source,
            model_params,
            |_l| {},
        )
        .unwrap_or_else(|err| {
            panic!("Failed to load {model_architecture} model from {model_path:?}: {err}")
        }),
    );

    tracing::info!(
        "{} - {} - fully loaded in: {}ms !",
        model_architecture,
        model_path.to_string_lossy(),
        now.elapsed().as_millis()
    );

    let model_list = ModelList {
        data: ["llama-2".into()],
    };

    let app = Router::new()
        .route("/v1/models", get(get_models))
        .with_state(model_list)
        .route("/v1/chat/completions", post(chat_completion))
        .route("/v1/completions", post(compat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/completions_full", post(completions))
        .route("/v1/completions_stream", post(completions_stream))
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .with_state(model)
        .layer(prometheus_layer)
        .layer(OtelAxumLayer::default());

    let host = config.host;
    let port = config.port;

    tracing::info!("listening on {host}:{port}");
    axum::Server::bind(&format!("{host}:{port}").as_str().parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

fn string_or_seq_string<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringOrVec(PhantomData<Vec<String>>);

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, visitor: S) -> Result<Self::Value, S::Error>
        where
            S: de::SeqAccess<'de>,
        {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(visitor))
        }
    }

    deserializer.deserialize_any(StringOrVec(PhantomData))
}
