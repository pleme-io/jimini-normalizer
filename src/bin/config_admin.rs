use clap::{Parser, Subcommand};
use jimini_normalizer::config;
use jimini_normalizer::db;
use jimini_normalizer::dynamic_config;

#[derive(Parser)]
#[command(name = "config-admin", about = "Dynamic config management CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all dynamic config entries
    List,
    /// Get a specific config entry
    Get {
        /// Config key (e.g. "enabled_providers", "retry.max_attempts")
        key: String,
    },
    /// Set (upsert) a config entry
    Set {
        /// Config key
        key: String,
        /// JSON value (e.g. '["provider_a"]', '5', '"hello"')
        value: String,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Delete a config entry
    Delete {
        /// Config key to delete
        key: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let cfg = config::load();

    let db_conn = db::connect(&cfg.database.url)
        .await
        .expect("failed to connect to database");

    db::run_migrations(&db_conn)
        .await
        .expect("failed to run migrations");

    match cli.command {
        Commands::List => {
            let entries = dynamic_config::list_entries(&db_conn)
                .await
                .expect("failed to list config");

            if entries.is_empty() {
                println!("No dynamic config entries.");
                return;
            }

            for entry in entries {
                println!(
                    "{:<40} {} (by {}, {})",
                    entry.key,
                    entry.value,
                    entry.updated_by,
                    entry.updated_at.format("%Y-%m-%d %H:%M:%S UTC"),
                );
                if let Some(desc) = &entry.description {
                    println!("  {}", desc);
                }
            }
        }
        Commands::Get { key } => {
            match dynamic_config::get_entry(&db_conn, &key).await {
                Ok(Some(entry)) => {
                    println!("Key:         {}", entry.key);
                    println!("Value:       {}", serde_json::to_string_pretty(&entry.value).unwrap());
                    if let Some(desc) = &entry.description {
                        println!("Description: {}", desc);
                    }
                    println!("Updated by:  {}", entry.updated_by);
                    println!("Updated at:  {}", entry.updated_at.format("%Y-%m-%d %H:%M:%S UTC"));
                }
                Ok(None) => {
                    eprintln!("Config key '{}' not found.", key);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Set { key, value, description } => {
            let json_value: serde_json::Value = serde_json::from_str(&value)
                .unwrap_or_else(|_| {
                    eprintln!("Error: '{}' is not valid JSON. Wrap strings in quotes: '\"hello\"'", value);
                    std::process::exit(1);
                });

            match dynamic_config::set_entry(&db_conn, &key, json_value, description, "config-admin-cli").await {
                Ok(entry) => {
                    println!("Set {} = {}", entry.key, entry.value);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Delete { key } => {
            match dynamic_config::delete_entry(&db_conn, &key).await {
                Ok(true) => println!("Deleted '{}'", key),
                Ok(false) => {
                    eprintln!("Config key '{}' not found.", key);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}
