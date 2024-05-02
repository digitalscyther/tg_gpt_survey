// $env:RUST_LOG="debug"; $env:TELOXIDE_TOKEN=""; cargo run
use log::*;
use rusqlite::{params, Connection, Result};
use serde_json::{json, Value};
use env_logger;
use teloxide::prelude::*;

const DEFAULT_PARAMS: [&str; 2] = ["Name", "Phone"];

async fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS survey (
            id INTEGER PRIMARY KEY,
            data TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS response (
            id INTEGER PRIMARY KEY,
            chat_id INTEGER NOT NULL,
            data TEXT NOT NULL
        )",
        [],
    )?;

    Ok(())
}

async fn change_or_create_survey(conn: &Connection, survey_data: &str) -> Result<()> {
    // Check if a survey already exists
    let existing_survey: Option<i64> = match conn.query_row(
        "SELECT id FROM survey ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
    ) {
        Ok(id) => Some(id),
        Err(_) => None,
    };

    if let Some(survey_id) = existing_survey {
        // Update existing survey
        conn.execute(
            "UPDATE survey SET data = ? WHERE id = ?",
            params![survey_data, survey_id],
        )?;
        info!("Survey updated successfully");
    } else {
        // Create new survey
        conn.execute(
            "INSERT INTO survey (data) VALUES (?)",
            params![survey_data],
        )?;
        info!("New survey created successfully");
    }

    Ok(())
}

async fn get_survey_params(conn: &Connection) -> Result<String> {
    let data: String = conn.query_row(
        "SELECT data FROM survey ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    Ok(data)
}

fn extract_params(json_string: &str) -> Option<Vec<String>> {
    // Parse the JSON string
    let parsed_json: Value = serde_json::from_str(json_string).ok()?;

    // Extract the "params" field as an array of strings
    let params_array = parsed_json.get("params")?;

    // Convert the array of values into a vector of strings
    let params: Vec<String> = params_array
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    Some(params)
}

async fn get_params(conn: &Connection) -> Result<Option<Vec<String>>> {
    let json_string = get_survey_params(&conn).await?;

    if let Some(params) = extract_params(&json_string) {
        return Ok(Some(params))
    }

    Ok(None)
}

async fn create_default_survey(conn: &Connection) -> Result<()> {
    let survey_data = json!({"params": DEFAULT_PARAMS}).to_string();
    match change_or_create_survey(&conn, &survey_data).await {
        Ok(_) => {}
        Err(err) => {
            eprintln!("Failed to create survey: {}", err);
            return Err(err);
        }
    }

    Ok(())
}

async fn prepare_db(conn: &Connection) -> Result<()> {
    // Attempt to create tables
    create_tables(&conn).await?;

    // Check params
    match get_params(&conn).await {
        Ok(Some(params)) => {
            info!("Existing survey - {:?}", params);
        }
        Ok(None) | Err(_) => {
            if let Err(err) = create_default_survey(&conn).await {
                eprintln!("Failed to create default survey: {}", err);
                return Err(err);
            }
            info!("Default survey created successfully");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // Attempt to open the database connection
    let conn = match Connection::open("db.sqlite") {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("Failed to open database connection: {}", err);
            return Err(err);
        }
    };

    if let Err(err) = prepare_db(&conn).await {
        eprintln!("Failed to prepare_db: {}", err);
        return Err(err);
    }

    let bot = Bot::from_env();

    teloxide::repl(bot, |bot: Bot, msg: Message| async move {
        bot.send_dice(msg.chat.id).await?;
        Ok(())
    })
    .await;

    Ok(())
}
