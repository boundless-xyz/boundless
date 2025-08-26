use crate::{AppError, AppState};
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::types::chrono;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use workflow_common::s3::S3Client;

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub timestamp: String,
    pub services: HashMap<String, ServiceHealth>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub status: ServiceStatus,
    pub response_time_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Up,
    Down,
    Degraded,
}

pub const HEALTH_PATH: &str = "/health";

pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HealthResponse>, AppError> {
    let mut services = HashMap::new();

    // Check Postgres
    services.insert("postgres".to_string(), check_postgres(&state.db_pool).await);

    // Check S3/MinIO
    services.insert("s3".to_string(), check_s3(&state.s3_client).await);

    let overall_status = determine_overall_health(&services);

    let response = HealthResponse {
        status: overall_status,
        timestamp: chrono::Utc::now().to_rfc3339(),
        services,
    };

    Ok(Json(response))
}

async fn check_postgres(pool: &PgPool) -> ServiceHealth {
    let start = Instant::now();

    match timeout(Duration::from_secs(5), async {
        sqlx::query("SELECT 1 as health_check").fetch_one(pool).await
    })
    .await
    {
        Ok(Ok(_)) => ServiceHealth {
            status: ServiceStatus::Up,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(Err(e)) => ServiceHealth {
            status: ServiceStatus::Down,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("Postgres error: {}", e)),
        },
        Err(_) => ServiceHealth {
            status: ServiceStatus::Down,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some("Postgres connection timeout".to_string()),
        },
    }
}

async fn check_s3(s3_client: &S3Client) -> ServiceHealth {
    let start = Instant::now();

    match timeout(Duration::from_secs(5), s3_client.bucket_exists()).await {
        Ok(Ok(true)) => ServiceHealth {
            status: ServiceStatus::Up,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(Ok(false)) => ServiceHealth {
            status: ServiceStatus::Degraded,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some("Bucket does not exist".to_string()),
        },
        Ok(Err(e)) => ServiceHealth {
            status: ServiceStatus::Down,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("S3/MinIO error: {}", e)),
        },
        Err(_) => ServiceHealth {
            status: ServiceStatus::Down,
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some("S3/MinIO connection timeout".to_string()),
        },
    }
}

fn determine_overall_health(services: &HashMap<String, ServiceHealth>) -> HealthStatus {
    if services.is_empty() {
        return HealthStatus::Healthy;
    }

    let down_count = services.values().filter(|s| matches!(s.status, ServiceStatus::Down)).count();

    let degraded_count =
        services.values().filter(|s| matches!(s.status, ServiceStatus::Degraded)).count();

    if down_count > 0 {
        if down_count == services.len() {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Degraded
        }
    } else if degraded_count > 0 {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}
