// $env:RUST_LOG="debug"; $env:TELOXIDE_TOKEN=""; cargo run
use dotenv::dotenv;
use env_logger;
use log::*;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;

const DEFAULT_PARAMS: [&str; 2] = ["Name", "Phone"];

async fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
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

async fn change_or_create_survey(conn: &Connection, survey_data: &str) -> Result<(), rusqlite::Error> {
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

async fn get_survey_data(conn: &Connection) -> Result<String, rusqlite::Error> {
    let data: String = conn.query_row(
        "SELECT data FROM survey ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    Ok(data)
}

fn extract_survey_config(json_string: &str) -> Option<SurveyConfig> {
    let config: SurveyConfig = serde_json::from_str(&json_string).ok()?;

    Some(config)
}

#[derive(Debug, Serialize, Deserialize)]
struct SurveyConfig {
    params: Vec<String>,
    prompt: String,
}

impl SurveyConfig {
    fn new(params: Vec<String>, prompt: String) -> Self {
        Self { params, prompt }
    }
}

async fn get_survey_config(conn: &Connection) -> Result<Option<SurveyConfig>> {
    let json_string = get_survey_data(&conn).await?;

    if let Some(survey_config) = extract_survey_config(&json_string) {
        return Ok(Some(survey_config));
    }

    Ok(None)
}

async fn create_default_survey(conn: &Connection) -> Result<(), rusqlite::Error> {
    let survey_config_data = get_default_survey_config();

    let survey_config_json = serde_json::to_string(&survey_config_data);

    let survey_config_str = match survey_config_json {
        Ok(value) => value,
        Err(_) => { panic!("Invalid survey_config_data") }
    };

    match change_or_create_survey(&conn, &survey_config_str).await {
        Ok(_) => {}
        Err(err) => {
            eprintln!("Failed to create survey: {}", err);
            return Err(err);
        }
    }

    Ok(())
}

fn get_default_survey_config() -> SurveyConfig {
    return SurveyConfig::new(
        DEFAULT_PARAMS.iter().map(|&s| s.into()).collect(),
        "hello".to_string(),
    );
}

async fn prepare_db() -> Result<(), rusqlite::Error> {
    let conn = match Connection::open("db.sqlite") {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("Failed to open database connection: {}", err);
            return Err(err);
        }
    };

    // Attempt to create tables
    create_tables(&conn).await?;

    // Check params
    match get_survey_config(&conn).await {
        Ok(Some(survey_config)) => {
            info!("Existing survey config - {:?}", survey_config);
        }
        Ok(None) | Err(_) => {
            if let Err(err) = create_default_survey(&conn).await {
                eprintln!("Failed to create default survey config: {}", err);
                return Err(err);
            }
            info!("Default survey config created successfully");
        }
    }

    Ok(())
}

// struct UserSurvey<'a> {
//     chat_id: ChatId,
//     survey_config: &'a SurveyConfig,
// }
//
// impl<'a> UserSurvey<'a> {
//     fn new(chat_id: ChatId, survey_config: &'a SurveyConfig) -> Self {
//         Self { chat_id, survey_config }
//     }
// }

// async fn process_chat(_conn: &Connection, chat_id: ChatId, user_text: String, bot: &Bot, survey_config: &SurveyConfig) -> Result<()> {
//     let _user_survey = UserSurvey::new(chat_id, &survey_config);
//
//     // user_survey.add_user_answer(user_text);
//
//     // let question = get_question(&user_survey, &survey_config.params);
//
//     // user_survey.add_assistant_question(question.clone());
//
//     if let Err(_) = bot.send_message(chat_id, user_text).await {
//         panic!("foo");
//     }
//
//     Ok(())
// }

// fn get_question(user_survey: &UserSurvey, prompt: &Vec<String>) -> String {
//     // TODO
//     "hello".to_string()
// }

async fn run_bot() -> Result<()> {
    let bot = Bot::from_env();

    teloxide::repl(bot, |bot: Bot, msg: Message| async move {
        match msg.text() {
            Some(text) => {
                match get_answer(text).await {
                    Ok(answer) => {
                        bot.send_message(msg.chat.id, answer).await?
                    }
                    Err(_) => {
                        bot.send_message(msg.chat.id, "Failed get answer").await?
                    }
                };
            }
            None => {
                bot.send_message(msg.chat.id, "Send me plain text.").await?;
            }
        }

        Ok(())
    }).await;

    Ok(())
}

async fn get_answer(user_text: &str) -> Result<String> {
    // let conn = match Connection::open("db.sqlite") {
    //     Ok(conn) => conn,
    //     Err(err) => {
    //         eprintln!("Failed to open database connection: {}", err);
    //         return Err(err);
    //     }
    // };
    //
    // let survey_config = match get_survey_config(&conn).await {
    //     Ok(Some(sc)) => sc,
    //     Ok(None) | Err(_) => {
    //         panic!("get_answer")
    //     }
    // };
    //
    // info!("get_answer config: {:?}", survey_config);

    Ok(format!("You asked: {}", user_text))
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    dotenv().ok();

    if let Err(err) = prepare_db().await {
        eprintln!("Failed to prepare_db: {}", err);
        return Err(err);
    }

    if let Err(err) = run_bot().await {
        eprintln!("Failed bot running: {}", err);
        return Err(err);
    }

    Ok(())
}
