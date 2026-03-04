use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use tracing::info;

pub async fn connect(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut opts = ConnectOptions::new(database_url);
    opts.max_connections(20)
        .min_connections(2)
        .sqlx_logging(false);

    let db = Database::connect(opts).await?;
    info!("connected to database");
    Ok(db)
}

pub async fn run_migrations(db: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(db, None).await?;
    info!("database migrations applied");
    Ok(())
}

pub async fn health_check(db: &DatabaseConnection) -> bool {
    use sea_orm::ConnectionTrait;
    db.execute_unprepared("SELECT 1").await.is_ok()
}
