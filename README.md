# MSSQL Audit LOgger

## Overview

MSSQL Audit Logger is a Rust-based audit collection service that reads Microsoft SQL Server audit files (`.sqlaudit`) through a central MSSQL instance and stores the parsed audit records in PostgreSQL.

The project is designed for centralized audit collection. Each MSSQL source can have its own audit file path, cluster name, server name, and server IP address. All records are written into common PostgreSQL tables and can be separated by metadata fields.

## Architecture

```text
MSSQL Audit Files (.sqlaudit)
        ↓
Central MSSQL Instance
        ↓
sys.fn_get_audit_file()
        ↓
Rust Collector
        ↓
PostgreSQL
```

The Rust collector does not parse `.sqlaudit` files directly from disk. Instead, it connects to a central MSSQL instance and uses:

```sql
sys.fn_get_audit_file()
```

Therefore, the audit file paths defined in `config.toml` must be readable by the MSSQL instance used for processing.

## Features

* Reads MSSQL `.sqlaudit` files
* Uses `sys.fn_get_audit_file()` for audit file reading
* Supports multiple MSSQL audit sources
* Stores database audit events in PostgreSQL
* Stores connection events separately in PostgreSQL
* Adds source metadata:

  * `cluster_name`
  * `server_name`
  * `server_ip`
* Uses per-source offset files
* Prevents duplicate inserts with `ON CONFLICT DO NOTHING`
* Runs as a Docker container
* Uses `config.toml` for all runtime configuration

## Project Structure

```text
.
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── config.toml
├── offsets/
└── src/
    └── main.rs
```

## Technologies

* Rust
* Docker
* Microsoft SQL Server
* PostgreSQL
* Tiberius
* SQLx
* Tokio
* TOML configuration

## Configuration

All runtime settings are defined in `config.toml`.

Before running the project, open the file and update it according to your own environment:

```bash
nano config.toml
```

Current configuration structure:

```toml
[central_mssql]
connection_string = "jdbc:sqlserver://MSSQL_HOST:MSSQL_POST;user=MSSQL_USER;password=MSSQL_PASSWORD;TrustServerCertificate=true;"

[postgres]
db_url = "postgres://POSTGRES_USER:POSTGRES_PASSWORD@POSTGRES_HOST:POSTGRES_PORT/POSTGRES_DB"

[collector]
poll_interval_secs = 5
sincedb_dir = "/app/offsets"

[[sources]]
cluster_name = "cluster-name"
server_name = "mssql-server-name"
server_ip = "server-ip-address"
audit_file_path = "/path/visible/to/mssql/*.sqlaudit"
```

### `[central_mssql]`

This section defines the MSSQL instance used to read audit files.

```toml
[central_mssql]
connection_string = "jdbc:sqlserver://MSSQL_HOST:MSSQL_PORT;user=MSSQL_USER;password=MSSQL_PASSWORD;TrustServerCertificate=true;"
```

Update the following values:

* `MSSQL_HOST`: MSSQL server address
*  `MSSQL_PORT`: MSSQL server port
* `MSSQL_USER`: MSSQL username
* `MSSQL_PASSWORD`: MSSQL password

The MSSQL instance configured here must be able to access the `.sqlaudit` file paths defined under `[[sources]]`.

### `[postgres]`

This section defines the PostgreSQL target database.

```toml
[postgres]
db_url = "postgres://POSTGRES_USER:POSTGRES_PASSWORD@POSTGRES_HOST:POSTGRES_PORT/POSTGRES_DB"
```

Update the following values:

* `POSTGRES_USER`: PostgreSQL username
* `POSTGRES_PASSWORD`: PostgreSQL password
* `POSTGRES_HOST`: PostgreSQL host address
* `POSTGRES_PORT`: PostgreSQL database port
* `POSTGRES_DB`: PostgreSQL database name

### `[collector]`

```toml
[collector]
poll_interval_secs = 5
sincedb_dir = "/app/offsets"
```

`poll_interval_secs` defines how often the collector scans audit sources.

`sincedb_dir` defines where offset files are stored inside the container.

The current Docker Compose file mounts the local `offsets` directory into the container:

```yaml
- ./offsets:/app/offsets
```

### `[[sources]]`

Each `[[sources]]` block represents one MSSQL audit source.

```toml
[[sources]]
cluster_name = "production-cluster"
server_name = "mssql-prod-01"
server_ip = "10.10.10.11"
audit_file_path = "/var/opt/mssql/audit-import/production/mssql-prod-01/*.sqlaudit"
```

Update the following values for each source:

* `cluster_name`: Logical cluster or environment name
* `server_name`: Source MSSQL server name
* `server_ip`: Source MSSQL server IP address
* `audit_file_path`: Path of `.sqlaudit` files for that source

The `audit_file_path` must be visible to the MSSQL instance configured in `[central_mssql]`.

For multiple sources, add multiple `[[sources]]` blocks:

```toml
[[sources]]
cluster_name = "production"
server_name = "mssql-prod-01"
server_ip = "10.10.10.11"
audit_file_path = "/var/opt/mssql/audit-import/production/mssql-prod-01/*.sqlaudit"

[[sources]]
cluster_name = "production"
server_name = "mssql-prod-02"
server_ip = "10.10.10.12"
audit_file_path = "/var/opt/mssql/audit-import/production/mssql-prod-02/*.sqlaudit"

[[sources]]
cluster_name = "test"
server_name = "mssql-test-01"
server_ip = "10.20.10.21"
audit_file_path = "/var/opt/mssql/audit-import/test/mssql-test-01/*.sqlaudit"
```

## Important Configuration Note

The project currently reads all settings from `config.toml`.

Before running the project, make sure that `config.toml` contains the correct values.

## PostgreSQL Schema

Create the required PostgreSQL tables before starting the collector.

### Connection Logs Table

```sql
CREATE TABLE IF NOT EXISTS mssql_connection_logs (
    id SERIAL PRIMARY KEY,
    log_time TIMESTAMPTZ NOT NULL,
    action_id CHAR(4) NOT NULL,
    succeeded BOOLEAN NOT NULL,
    server_principal_name VARCHAR(256) NOT NULL,
    client_ip VARCHAR(45),
    application_name VARCHAR(256),
    session_id INT,
    cluster_name VARCHAR(100),
    server_name VARCHAR(100),
    server_ip VARCHAR(45)
);
```

### Audit Logs Table

```sql
CREATE TABLE IF NOT EXISTS mssql_audit_logs (
    id SERIAL PRIMARY KEY,
    log_time TIMESTAMPTZ NOT NULL,
    action_id CHAR(4) NOT NULL,
    succeeded BOOLEAN NOT NULL,
    server_principal_name VARCHAR(256) NOT NULL,
    database_name VARCHAR(256),
    schema_name VARCHAR(256),
    object_name VARCHAR(256),
    statement TEXT,
    client_ip VARCHAR(45),
    application_name VARCHAR(256),
    session_id INT,
    cluster_name VARCHAR(100),
    server_name VARCHAR(100),
    server_ip VARCHAR(45)
);
```

## Recommended Indexes

```sql
CREATE INDEX IF NOT EXISTS idx_mssql_connection_log_time
ON mssql_connection_logs (log_time DESC);

CREATE INDEX IF NOT EXISTS idx_mssql_audit_log_time
ON mssql_audit_logs (log_time DESC);

CREATE INDEX IF NOT EXISTS idx_mssql_connection_server_time
ON mssql_connection_logs (server_name, log_time DESC);

CREATE INDEX IF NOT EXISTS idx_mssql_audit_server_time
ON mssql_audit_logs (server_name, log_time DESC);

CREATE INDEX IF NOT EXISTS idx_mssql_audit_cluster_time
ON mssql_audit_logs (cluster_name, log_time DESC);
```

## Duplicate Prevention

The collector inserts records using `ON CONFLICT DO NOTHING`.

To make duplicate prevention effective, create unique indexes.

### Connection Logs Unique Index

```sql
CREATE UNIQUE INDEX IF NOT EXISTS uq_mssql_connection_logs_dedup
ON mssql_connection_logs (
    log_time,
    action_id,
    succeeded,
    server_principal_name,
    COALESCE(client_ip, ''),
    COALESCE(application_name, ''),
    COALESCE(session_id, -1),
    COALESCE(cluster_name, ''),
    COALESCE(server_name, ''),
    COALESCE(server_ip, '')
);
```

### Audit Logs Unique Index

```sql
CREATE UNIQUE INDEX IF NOT EXISTS uq_mssql_audit_logs_dedup
ON mssql_audit_logs (
    log_time,
    action_id,
    succeeded,
    server_principal_name,
    COALESCE(database_name, ''),
    COALESCE(schema_name, ''),
    COALESCE(object_name, ''),
    COALESCE(statement, ''),
    COALESCE(client_ip, ''),
    COALESCE(application_name, ''),
    COALESCE(session_id, -1),
    COALESCE(cluster_name, ''),
    COALESCE(server_name, ''),
    COALESCE(server_ip, '')
);
```

## MSSQL Audit Setup

Audit configuration may differ between organizations. The following examples show a general setup.

### Create Server Audit

```sql
USE master;
GO

CREATE SERVER AUDIT EnterpriseServerAudit
TO FILE (
    FILEPATH = '/path/to/mssql/audit/',
    MAXSIZE = 100 MB,
    MAX_ROLLOVER_FILES = 20,
    RESERVE_DISK_SPACE = OFF
)
WITH (
    QUEUE_DELAY = 1000,
    ON_FAILURE = CONTINUE
);
GO

ALTER SERVER AUDIT EnterpriseServerAudit
WITH (STATE = ON);
GO
```

Replace `/path/to/mssql/audit/` with the actual MSSQL audit directory.

### Create Database Audit Specification

Replace `YourDatabaseName` with the database that should be audited.

```sql
USE YourDatabaseName;
GO

CREATE DATABASE AUDIT SPECIFICATION DatabaseAuditSpecification
FOR SERVER AUDIT EnterpriseServerAudit
ADD (SELECT ON DATABASE::YourDatabaseName BY public),
ADD (INSERT ON DATABASE::YourDatabaseName BY public),
ADD (UPDATE ON DATABASE::YourDatabaseName BY public),
ADD (DELETE ON DATABASE::YourDatabaseName BY public),
ADD (SCHEMA_OBJECT_CHANGE_GROUP),
ADD (DATABASE_OBJECT_CHANGE_GROUP)
WITH (STATE = ON);
GO
```

If only a specific user or role should be audited, replace `public` with that user or role.

Example:

```sql
ADD (SELECT ON DATABASE::YourDatabaseName BY YourUserName)
```

### Create Login Audit Specification

```sql
USE master;
GO

CREATE SERVER AUDIT SPECIFICATION LoginAuditSpecification
FOR SERVER AUDIT EnterpriseServerAudit
ADD (SUCCESSFUL_LOGIN_GROUP),
ADD (FAILED_LOGIN_GROUP),
ADD (LOGOUT_GROUP)
WITH (STATE = ON);
GO
```

## File Permission Requirements

The audit files are read by MSSQL through:

```sql
sys.fn_get_audit_file()
```

This means the MSSQL service account must have read permission on the audit file directory.

Example Linux permission setup:

```bash
sudo mkdir -p /var/opt/mssql/audit-import/server-01
sudo chown -R mssql:mssql /var/opt/mssql/audit-import
sudo chmod -R 750 /var/opt/mssql/audit-import
```

Avoid using user home directories for audit files unless the MSSQL service account has the required permissions.

## Docker Deployment

The project includes a `Dockerfile` and `docker-compose.yml`.

The Docker image is built using Rust and then runs the compiled binary on a minimal Debian image.

Current Docker Compose structure:

```yaml
services:
  mssql-audit-logger:
    build: .
    container_name: mssql-audit-logger
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./config.toml:/app/config.toml:ro
      - ./offsets:/app/offsets
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

The container reads configuration from:

```text
/app/config.toml
```

The local file is mounted as:

```yaml
- ./config.toml:/app/config.toml:ro
```

The offset directory is mounted as:

```yaml
- ./offsets:/app/offsets
```

## Running the Project

Create the offset directory if it does not exist:

```bash
mkdir -p offsets
```

Build and start the collector:

```bash
docker compose up -d --build
```

View logs:

```bash
docker logs -f mssql-audit-logger
```

Expected output example:

```text
PostgreSQL connection successful.
[production / mssql-prod-01] Audit: 10 new record(s), Connection: 2 new record(s).
Scan completed. Total Audit: 10, Total Connection: 2.
```

## Offset Tracking

The collector creates one offset file per source.

Offset file names are generated from:

```text
cluster_name + server_name
```

Example:

```text
/app/offsets/production_mssql-prod-01.offset
/app/offsets/production_mssql-prod-02.offset
```

Each offset file stores the last processed `event_time`.

If you want to reprocess audit files during testing, stop the container and delete the offset files:

```bash
docker compose stop mssql-audit-logger
rm -f offsets/*.offset
docker compose up -d --build
```

## Log Classification

The collector separates audit events into two groups.

### Connection Events

The following MSSQL action IDs are stored in `mssql_connection_logs`:

```text
LGIS
LGIF
LGO
```

### Database Audit Events

All other non-filtered events are stored in `mssql_audit_logs`.

## Built-in Filtering

The collector skips some internal or noisy records.

Skipped examples:

* Empty `server_principal_name`
* `NT AUTHORITY` records
* Collector’s own `sys.fn_get_audit_file()` queries
* Tiberius client noise
* Some DBeaver metadata queries, except SQL editor activity

This prevents the collector from storing its own audit-reading queries and unnecessary metadata noise.

## Checking Results in PostgreSQL

### Recent Audit Logs

```sql
SELECT *
FROM mssql_audit_logs
ORDER BY log_time DESC
LIMIT 20;
```

### Recent Connection Logs

```sql
SELECT *
FROM mssql_connection_logs
ORDER BY log_time DESC
LIMIT 20;
```

### Filter by Server

```sql
SELECT *
FROM mssql_audit_logs
WHERE server_name = 'mssql-prod-01'
ORDER BY log_time DESC;
```

### Filter by Database

```sql
SELECT *
FROM mssql_audit_logs
WHERE database_name = 'YourDatabaseName'
ORDER BY log_time DESC;
```

### Count by Source Server

```sql
SELECT server_name, COUNT(*)
FROM mssql_audit_logs
GROUP BY server_name
ORDER BY server_name;
```

### Count by Action

```sql
SELECT action_id, COUNT(*)
FROM mssql_audit_logs
GROUP BY action_id
ORDER BY COUNT(*) DESC;
```

## Testing Audit Collection

Use your own audited database and table names.

Example:

```sql
USE YourDatabaseName;
GO

CREATE TABLE AuditTestTable (
    id INT PRIMARY KEY,
    name NVARCHAR(100)
);
GO

INSERT INTO AuditTestTable VALUES (1, N'test');

SELECT * FROM AuditTestTable;

UPDATE AuditTestTable
SET name = N'updated'
WHERE id = 1;

DELETE FROM AuditTestTable
WHERE id = 1;

ALTER TABLE AuditTestTable
ADD description NVARCHAR(200);
GO
```

Then check PostgreSQL:

```sql
SELECT *
FROM mssql_audit_logs
ORDER BY log_time DESC
LIMIT 20;
```

## Troubleshooting

### No records are inserted

First, test if MSSQL can read the audit files:

```sql
SELECT TOP 10 *
FROM sys.fn_get_audit_file('/path/to/audit/files/*.sqlaudit', DEFAULT, DEFAULT);
```

If this query returns no rows, the collector will also return no rows.

### MSSQL cannot read audit files

Check directory permissions:

```bash
sudo chown -R mssql:mssql /path/to/audit/files
sudo chmod -R 750 /path/to/audit/files
```

### Wrong audit file path

Make sure the `audit_file_path` in `config.toml` is correct and visible to MSSQL.

```toml
audit_file_path = "/path/visible/to/mssql/*.sqlaudit"
```

### PostgreSQL connection error

Check the `[postgres]` section in `config.toml`:

```toml
[postgres]
db_url = "postgres://POSTGRES_USER:POSTGRES_PASSWORD@POSTGRES_HOST:5432/POSTGRES_DB"
```

### MSSQL connection error

Check the `[central_mssql]` section in `config.toml`:

```toml
[central_mssql]
connection_string = "jdbc:sqlserver://MSSQL_HOST:1433;user=MSSQL_USER;password=MSSQL_PASSWORD;TrustServerCertificate=true;"
```

### Reprocessing old files

Delete offset files and restart the container:

```bash
docker compose stop mssql-audit-logger
rm -f offsets/*.offset
docker compose up -d --build
```

### Duplicate records

Make sure the unique indexes are created and the collector uses `ON CONFLICT DO NOTHING`.

## Security Notes

* Do not commit real production credentials to a public repository.
* `config.toml` contains database connection strings.
* Restrict file permissions for `config.toml`.
* Restrict access to audit directories.
* Restrict PostgreSQL access to trusted hosts.
* Use database users with the minimum required permissions.
* Audit logs may contain sensitive SQL statements and user information.

Recommended local permission:

```bash
chmod 600 config.toml
```

## Notes

* The project is configured using `config.toml`.
* Environment variables are not required by the current implementation.
* Each source has its own offset file.
* Audit file paths can be different for every source.
* Database names are not hardcoded.
* All records are stored in common PostgreSQL tables.
* Source metadata is used to distinguish logs from different MSSQL servers.
* The current offset strategy is based on the latest processed `event_time`.
* If late-arriving audit files contain older timestamps, a per-file offset strategy may be added in future versions.

## License

This project is provided for audit collection and educational purposes. Review and adapt the configuration before production use.
