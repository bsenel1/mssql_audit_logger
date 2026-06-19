use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use serde::Deserialize;
use sqlx::PgPool;
use std::fs;
use std::path::Path;
use tiberius::{Client, Config};
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    central_mssql: CentralMssqlConfig,
    postgres: PostgresConfig,
    collector: CollectorConfig,
    sources: Vec<SourceConfig>,
}

#[derive(Debug, Deserialize, Clone)]
struct CentralMssqlConfig {
    connection_string: String,
}

#[derive(Debug, Deserialize, Clone)]
struct PostgresConfig {
    db_url: String,
}

#[derive(Debug, Deserialize, Clone)]
struct CollectorConfig {
    poll_interval_secs: u64,
    sincedb_dir: String,
}

#[derive(Debug, Deserialize, Clone)]
struct SourceConfig {
    cluster_name: String,
    server_name: String,
    server_ip: String,
    audit_file_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config("/app/config.toml")?;

    fs::create_dir_all(&config.collector.sincedb_dir)?;

    let pg_pool = PgPool::connect(&config.postgres.db_url).await?;
    println!("PostgreSQL connection successful.");

    loop {
        let mut total_audit_inserted: i64 = 0;
        let mut total_connection_inserted: i64 = 0;

        for source in &config.sources {
            match process_source(&config, source, &pg_pool).await {
                Ok((audit_count, connection_count)) => {
                    total_audit_inserted += audit_count;
                    total_connection_inserted += connection_count;

                    println!(
                        "[{} / {}] Audit: {} new record, Connection: {} new record.",
                        source.cluster_name,
                        source.server_name,
                        audit_count,
                        connection_count
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[{} / {}] Error occured: {}",
                        source.cluster_name,
                        source.server_name,
                        e
                    );
                }
            }
        }

        println!(
            "Scan completed. Total Audit: {}, Total Connection: {}.",
            total_audit_inserted,
            total_connection_inserted
        );

        sleep(Duration::from_secs(config.collector.poll_interval_secs)).await;
    }
}

fn load_config(path: &str) -> Result<AppConfig, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: AppConfig = toml::from_str(&content)?;
    Ok(config)
}

async fn create_mssql_client(
    config: &AppConfig,
) -> Result<Client<Compat<TcpStream>>, Box<dyn std::error::Error>> {
    let mssql_config = Config::from_jdbc_string(&config.central_mssql.connection_string)?;

    let tcp = TcpStream::connect(mssql_config.get_addr()).await?;
    tcp.set_nodelay(true)?;

    let client = Client::connect(mssql_config, tcp.compat_write()).await?;

    Ok(client)
}

async fn process_source(
    config: &AppConfig,
    source: &SourceConfig,
    pg_pool: &PgPool,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let mut mssql_client = create_mssql_client(config).await?;

    let offset_path = build_offset_path(&config.collector.sincedb_dir, source);
    let last_log_time = read_last_log_time_from_offset(&offset_path);

    let audit_query = format!(
        "
        SELECT
            event_time,
            action_id,
            succeeded,
            server_principal_name,
            database_name,
            schema_name,
            object_name,
            statement,
            client_ip,
            application_name,
            session_id
        FROM sys.fn_get_audit_file('{}', DEFAULT, DEFAULT)
        WHERE event_time >= @P1
        ORDER BY event_time ASC;
        ",
        source.audit_file_path.replace('\'', "''")
    );

    let mut stream = mssql_client.query(&audit_query, &[&last_log_time]).await?;

    let mut audit_inserted_count: i64 = 0;
    let mut connection_inserted_count: i64 = 0;
    let mut max_processed_log_time: Option<DateTime<Utc>> = None;

    while let Some(item) = stream.next().await {
        let item = item?;

        if let tiberius::QueryItem::Row(row) = item {
            let log_time: DateTime<Utc> = match row.get(0) {
                Some(value) => value,
                None => continue,
            };

            update_max_log_time(&mut max_processed_log_time, log_time);

            let action_id: &str = row.get(1).unwrap_or("");
            let succeeded: bool = row.get(2).unwrap_or(false);
            let server_principal_name: &str = row.get(3).unwrap_or("");
            let database_name: &str = row.get(4).unwrap_or("");
            let schema_name: &str = row.get(5).unwrap_or("");
            let object_name: &str = row.get(6).unwrap_or("");
            let statement: &str = row.get(7).unwrap_or("");
            let client_ip: &str = row.get(8).unwrap_or("");
            let application_name: &str = row.get(9).unwrap_or("");
            let session_id: i16 = row.get(10).unwrap_or(0);

            if should_skip_log(server_principal_name, statement, application_name) {
                continue;
            }

            if is_connection_action(action_id) {
                let inserted = insert_connection_log(
                    pg_pool,
                    source,
                    log_time,
                    action_id,
                    succeeded,
                    server_principal_name,
                    client_ip,
                    application_name,
                    session_id as i32,
                )
                .await?;

                if inserted {
                    connection_inserted_count += 1;
                }
            } else {
                let inserted = insert_audit_log(
                    pg_pool,
                    source,
                    log_time,
                    action_id,
                    succeeded,
                    server_principal_name,
                    database_name,
                    schema_name,
                    object_name,
                    statement,
                    client_ip,
                    application_name,
                    session_id as i32,
                )
                .await?;

                if inserted {
                    audit_inserted_count += 1;
                }
            }
        }
    }

    if let Some(last_time) = max_processed_log_time {
        write_last_log_time_to_offset(&offset_path, last_time)?;
    }

    Ok((audit_inserted_count, connection_inserted_count))
}

fn is_connection_action(action_id: &str) -> bool {
    matches!(action_id.trim(), "LGIS" | "LGIF" | "LGO")
}

fn should_skip_log(server_principal_name: &str, statement: &str, application_name: &str) -> bool {
    if server_principal_name.trim().is_empty() {
        return true;
    }

    if server_principal_name.contains("NT AUTHORITY") {
        return true;
    }

    let statement_upper = statement.to_uppercase();
    let application_lower = application_name.to_lowercase();

    if application_lower.contains("tiberius") {
        return true;
    }

    if statement_upper.contains("FN_GET_AUDIT_FILE") {
        return true;
    }

    if application_lower.contains("dbeaver") && !application_lower.contains("sqleditor") {
        return true;
    }

    false
}

async fn insert_connection_log(
    pg_pool: &PgPool,
    source: &SourceConfig,
    log_time: DateTime<Utc>,
    action_id: &str,
    succeeded: bool,
    server_principal_name: &str,
    client_ip: &str,
    application_name: &str,
    session_id: i32,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "
        INSERT INTO mssql_connection_logs
        (
            log_time,
            action_id,
            succeeded,
            server_principal_name,
            client_ip,
            application_name,
            session_id,
            cluster_name,
            server_name,
            server_ip
        )
        VALUES
        ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT DO NOTHING
        ",
    )
    .bind(log_time)
    .bind(action_id)
    .bind(succeeded)
    .bind(server_principal_name)
    .bind(empty_to_none(client_ip))
    .bind(empty_to_none(application_name))
    .bind(session_id)
    .bind(&source.cluster_name)
    .bind(&source.server_name)
    .bind(&source.server_ip)
    .execute(pg_pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

async fn insert_audit_log(
    pg_pool: &PgPool,
    source: &SourceConfig,
    log_time: DateTime<Utc>,
    action_id: &str,
    succeeded: bool,
    server_principal_name: &str,
    database_name: &str,
    schema_name: &str,
    object_name: &str,
    statement: &str,
    client_ip: &str,
    application_name: &str,
    session_id: i32,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "
        INSERT INTO mssql_audit_logs
        (
            log_time,
            action_id,
            succeeded,
            server_principal_name,
            database_name,
            schema_name,
            object_name,
            statement,
            client_ip,
            application_name,
            session_id,
            cluster_name,
            server_name,
            server_ip
        )
        VALUES
        ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT DO NOTHING
        ",
    )
    .bind(log_time)
    .bind(action_id)
    .bind(succeeded)
    .bind(server_principal_name)
    .bind(empty_to_none(database_name))
    .bind(empty_to_none(schema_name))
    .bind(empty_to_none(object_name))
    .bind(empty_to_none(statement))
    .bind(empty_to_none(client_ip))
    .bind(empty_to_none(application_name))
    .bind(session_id)
    .bind(&source.cluster_name)
    .bind(&source.server_name)
    .bind(&source.server_ip)
    .execute(pg_pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

fn empty_to_none(value: &str) -> Option<&str> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn build_offset_path(sincedb_dir: &str, source: &SourceConfig) -> String {
    let file_name = format!(
        "{}_{}.offset",
        sanitize_file_name(&source.cluster_name),
        sanitize_file_name(&source.server_name)
    );

    Path::new(sincedb_dir)
        .join(file_name)
        .to_string_lossy()
        .to_string()
}

fn sanitize_file_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn read_last_log_time_from_offset(path: &str) -> DateTime<Utc> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim();

            if trimmed.is_empty() {
                return default_start_time();
            }

            match DateTime::parse_from_rfc3339(trimmed) {
                Ok(dt) => dt.with_timezone(&Utc),
                Err(_) => default_start_time(),
            }
        }
        Err(_) => default_start_time(),
    }
}

fn write_last_log_time_to_offset(
    path: &str,
    log_time: DateTime<Utc>,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, log_time.to_rfc3339())?;
    Ok(())
}

fn update_max_log_time(current: &mut Option<DateTime<Utc>>, new_time: DateTime<Utc>) {
    match current {
        Some(existing_time) => {
            if new_time > *existing_time {
                *current = Some(new_time);
            }
        }
        None => {
            *current = Some(new_time);
        }
    }
}

fn default_start_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
