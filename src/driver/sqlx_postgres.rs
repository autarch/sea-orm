use std::{future::Future, pin::Pin};

use sqlx::{
    postgres::{PgArguments, PgConnectOptions, PgQueryResult, PgRow},
    PgPool, Postgres,
};

sea_query::sea_query_driver_postgres!();
use sea_query_driver_postgres::bind_query;

use crate::{
    debug_print, error::*, executor::*, ConnectOptions, DatabaseConnection, DatabaseTransaction,
    QueryStream, Statement, TransactionError,
};

use super::sqlx_common::*;

/// Defines the [sqlx::postgres] connector
#[derive(Debug)]
pub struct SqlxPostgresConnector;

/// Defines a sqlx PostgreSQL pool
#[derive(Debug, Clone)]
pub struct SqlxPostgresPoolConnection {
    pool: PgPool,
}

impl SqlxPostgresConnector {
    /// Check if the URI provided corresponds to `postgres://` for a PostgreSQL database
    pub fn accepts(string: &str) -> bool {
        string.starts_with("postgres://") && string.parse::<PgConnectOptions>().is_ok()
    }

    /// Add configuration options for the MySQL database
    pub async fn connect(options: ConnectOptions) -> Result<DatabaseConnection, DbErr> {
        let mut opt = options
            .url
            .parse::<PgConnectOptions>()
            .map_err(|e| DbErr::Conn(e.to_string()))?;
        if !options.sqlx_logging {
            use sqlx::ConnectOptions;
            opt.disable_statement_logging();
        }
        match options.pool_options().connect_with(opt).await {
            Ok(pool) => Ok(DatabaseConnection::SqlxPostgresPoolConnection(
                SqlxPostgresPoolConnection { pool },
            )),
            Err(e) => Err(sqlx_error_to_conn_err(e)),
        }
    }
}

impl SqlxPostgresConnector {
    /// Instantiate a sqlx pool connection to a [DatabaseConnection]
    pub fn from_sqlx_postgres_pool(pool: PgPool) -> DatabaseConnection {
        DatabaseConnection::SqlxPostgresPoolConnection(SqlxPostgresPoolConnection { pool })
    }
}

impl SqlxPostgresPoolConnection {
    /// Execute a [Statement] on a PostgreSQL backend
    pub async fn execute(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        if let Ok(conn) = &mut self.pool.acquire().await {
            match query.execute(conn).await {
                Ok(res) => Ok(res.into()),
                Err(err) => Err(sqlx_error_to_exec_err(err)),
            }
        } else {
            Err(DbErr::Exec(
                "Failed to acquire connection from pool.".to_owned(),
            ))
        }
    }

    /// Get one result from a SQL query. Returns [Option::None] if no match was found
    pub async fn query_one(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        if let Ok(conn) = &mut self.pool.acquire().await {
            match query.fetch_one(conn).await {
                Ok(row) => Ok(Some(row.into())),
                Err(err) => match err {
                    sqlx::Error::RowNotFound => Ok(None),
                    _ => Err(DbErr::Query(err.to_string())),
                },
            }
        } else {
            Err(DbErr::Query(
                "Failed to acquire connection from pool.".to_owned(),
            ))
        }
    }

    /// Get the results of a query returning them as a Vec<[QueryResult]>
    pub async fn query_all(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        debug_print!("{}", stmt);

        let query = sqlx_query(&stmt);
        if let Ok(conn) = &mut self.pool.acquire().await {
            match query.fetch_all(conn).await {
                Ok(rows) => Ok(rows.into_iter().map(|r| r.into()).collect()),
                Err(err) => Err(sqlx_error_to_query_err(err)),
            }
        } else {
            Err(DbErr::Query(
                "Failed to acquire connection from pool.".to_owned(),
            ))
        }
    }

    /// Stream the results of executing a SQL query
    pub async fn stream(&self, stmt: Statement) -> Result<QueryStream, DbErr> {
        debug_print!("{}", stmt);

        if let Ok(conn) = self.pool.acquire().await {
            Ok(QueryStream::from((conn, stmt)))
        } else {
            Err(DbErr::Query(
                "Failed to acquire connection from pool.".to_owned(),
            ))
        }
    }

    /// Bundle a set of SQL statements that execute together.
    pub async fn begin(&self) -> Result<DatabaseTransaction, DbErr> {
        if let Ok(conn) = self.pool.acquire().await {
            DatabaseTransaction::new_postgres(conn).await
        } else {
            Err(DbErr::Query(
                "Failed to acquire connection from pool.".to_owned(),
            ))
        }
    }

    /// Create a PostgreSQL transaction
    pub async fn transaction<F, T, E>(&self, callback: F) -> Result<T, TransactionError<E>>
    where
        F: for<'b> FnOnce(
                &'b DatabaseTransaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'b>>
            + Send,
        T: Send,
        E: std::error::Error + Send,
    {
        if let Ok(conn) = self.pool.acquire().await {
            let transaction = DatabaseTransaction::new_postgres(conn)
                .await
                .map_err(|e| TransactionError::Connection(e))?;
            transaction.run(callback).await
        } else {
            Err(TransactionError::Connection(DbErr::Query(
                "Failed to acquire connection from pool.".to_owned(),
            )))
        }
    }
}

impl From<PgRow> for QueryResult {
    fn from(row: PgRow) -> QueryResult {
        QueryResult {
            row: QueryResultRow::SqlxPostgres(row),
        }
    }
}

impl From<PgQueryResult> for ExecResult {
    fn from(result: PgQueryResult) -> ExecResult {
        ExecResult {
            result: ExecResultHolder::SqlxPostgres(result),
        }
    }
}

pub(crate) fn sqlx_query(stmt: &Statement) -> sqlx::query::Query<'_, Postgres, PgArguments> {
    let mut query = sqlx::query(&stmt.sql);
    if let Some(values) = &stmt.values {
        query = bind_query(query, values);
    }
    query
}
