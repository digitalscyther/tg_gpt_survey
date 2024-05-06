// $env:RUST_LOG="debug"; $env:TELOXIDE_TOKEN=""; cargo run
use std::env;
use dotenv::dotenv;
use env_logger;
use log::*;
use reqwest::header::HeaderMap;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;


const DB_NAME: &str = "db.sqlite";
const USER_TABLE_NAME: &str = "user";
const DEFAULT_PARAMS: [&str; 2] = ["Name", "Phone"];
const DEFAULT_PROMPT: &str = "hello";

async fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS survey (
            id INTEGER PRIMARY KEY,
            data TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS user (
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

fn get_survey_data(conn: &Connection) -> Result<String, rusqlite::Error> {
    let data: String = conn.query_row(
        "SELECT data FROM survey ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    Ok(data)
}

fn get_user_data_json(chat_id: i64, conn: &Connection) -> Result<String, rusqlite::Error> {
    let data: String = conn.query_row(
        "SELECT data FROM user WHERE chat_id = ? ORDER BY id LIMIT 1",
        params![chat_id],
        |row| row.get(0),
    )?;
    Ok(data)
}

fn clear_user_table(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        &format!("DELETE FROM {}", USER_TABLE_NAME),
        [],
    )?;
    Ok(())
}

fn extract_survey_config(json_string: &str) -> Option<SurveyConfig> {
    let config: SurveyConfig = serde_json::from_str(&json_string).ok()?;

    Some(config)
}

fn extract_user_data(json_string: &str) -> Option<UserData> {
    let user_data: UserData = serde_json::from_str(&json_string).ok()?;

    Some(user_data)
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

fn get_survey_config(conn: &Connection) -> Option<SurveyConfig> {
    let json_string = get_survey_data(&conn).ok()?;

    if let Some(survey_config) = extract_survey_config(&json_string) {
        return Some(survey_config);
    }

    None
}

fn get_user_data(
    chat_id: i64,
    params: &Vec<String>,
    conn: &Connection,
    or_default: bool,
) -> (Option<UserData>, bool) {
    match get_user_data_json(chat_id, conn) {
        Ok(json_string) => {
            if let Some(user_data) = extract_user_data(&json_string) {
                (Some(user_data), false)
            } else if or_default {
                (Some(UserData::default(params)), true)
            } else {
                (None, false)
            }
        }
        Err(_) if or_default => (Some(UserData::default(params)), true),
        _ => (None, false),
    }
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
        DEFAULT_PROMPT.to_string(),
    );
}

async fn prepare_db(conn: &Connection) -> Result<(), rusqlite::Error> {
    create_tables(&conn).await?;

    match get_survey_config(&conn) {
        Some(survey_config) => {
            info!("Existing survey config - {:?}", survey_config);
        }
        _ => {
            if let Err(err) = create_default_survey(&conn).await {
                eprintln!("Failed to create default survey config: {}", err);
                return Err(err);
            }
            info!("Default survey config created successfully");
        }
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct Param {
    index: u16,
    name: String,
    value: Option<String>,
}


#[derive(Debug, Serialize, Deserialize)]
struct GptMessage {
    role: String,
    content: String,
}


#[derive(Debug, Serialize, Deserialize)]
struct UserData {
    params: Vec<Param>,
    messages: Vec<GptMessage>,
}

impl UserData {
    fn default(params: &Vec<String>) -> Self {
        let params = params
            .into_iter()
            .enumerate()
            .map(|(index, name)| Param {
                index: index as u16,
                name: name.clone(),
                value: None,
            })
            .collect();

        UserData {
            params,
            messages: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct UserSurvey {
    chat_id: i64,
    survey_config: SurveyConfig,
    _data: UserData,
}

impl UserSurvey {
    fn new(chat_id: i64, survey_config: SurveyConfig, conn: &Connection) -> Self {
        let _data = get_user_data(chat_id, &survey_config.params, conn, true).0.expect("foo");
        Self { chat_id, survey_config, _data }
    }

    fn add_user_answer(&mut self, text: &str) {
        self.add_message("user", text);
    }

    fn add_assistant_question(&mut self, text: &str) {
        self.add_message("assistant", text);
    }

    fn add_message(&mut self, role: &str, content: &str) {
        self._data.messages.push(GptMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
    }

    fn sync_data(&mut self, conn: &Connection) -> Result<()> {
        let data_str = serde_json::to_string(&self._data).expect("Serialization failed");

        match get_user_data(self.chat_id, &self.survey_config.params, &conn, false) {
            (None, _) => {
                conn.execute(
                    &format!(
                        "INSERT INTO {} (chat_id, data) VALUES (?, ?)",
                        USER_TABLE_NAME
                    ),
                    params![self.chat_id, data_str],
                )?;
            }
            (Some(_), _) => {
                conn.execute(
                    &format!(
                        "UPDATE {} SET data = ? WHERE chat_id = ?",
                        USER_TABLE_NAME
                    ),
                    params![data_str, self.chat_id],
                )?;
            }
        }

        Ok(())
    }
}

type MyDialogue = Dialogue<State, InMemStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Default)]
enum State {
    #[default]
    Answer,
}


async fn run_bot() -> Result<()> {
    let bot = Bot::from_env();

    Dispatcher::builder(
        bot,
        Update::filter_message()
            .enter_dialogue::<Message, InMemStorage<State>, State>()
            .branch(dptree::case![State::Answer].endpoint(answer)),
    )
        .dependencies(dptree::deps![InMemStorage::<State>::new()])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn answer(bot: Bot, _dialogue: MyDialogue, msg: Message) -> HandlerResult {
    let text = match msg.text() {
        Some(text) => text,
        _ => {
            bot.send_dice(msg.chat.id).await?;
            return Ok(());
        }
    };

    let conn = establish_connection()?;
    let survey_config: SurveyConfig = get_survey_config(&conn).unwrap();
    let mut user_survey = UserSurvey::new(msg.chat.id.0, survey_config, &conn);

    if text == "/clear_user_table" {
        clear_user_table(&conn).unwrap();
        bot.send_message(msg.chat.id, "User table cleared").await?;
        return Ok(());
    }

    if text == "/export_csv" {
        // TODO
        bot.send_message(msg.chat.id, "TODO 1").await?;
        return Ok(());
    }

    if text == "/start" || text == "/help" {
        // TODO
        bot.send_message(msg.chat.id, "TODO 2").await?;
        return Ok(());
    }

    if text.starts_with("/prompt") {
        // TODO
        bot.send_message(msg.chat.id, "TODO 3").await?;
        return Ok(());
    }

    if text.starts_with("/params") {
        // TODO
        bot.send_message(msg.chat.id, "TODO 4").await?;
        return Ok(());
    }

    user_survey.add_user_answer(text);
    let question = get_question(&user_survey).await.expect("foo");

    bot.send_message(msg.chat.id, &question).await?;

    user_survey.add_assistant_question(question.as_str());
    user_survey.sync_data(&conn).ok();
    info!("{:?}", user_survey);

    Ok(())
}

async fn get_question(user_survey: &UserSurvey) -> Result<String> {
    let openai_resp = get_openai_question(&user_survey._data.messages, &user_survey.survey_config.params).await.expect("foo");
    info!("{:?}", openai_resp);

    Ok(format!(
        "What's your {}?",
        user_survey._data.params.first().map(|f| &f.name).unwrap_or(&"parameter".to_string())
    ))
}


#[derive(Debug)]
enum GptAnswer {
    Question(String),
    FunctionCallArgs(Vec<FunctionArg>),
}

#[derive(Debug)]
struct FunctionArg {
    index: i32,
    value: String,
}

async fn get_openai_question(messages: &Vec<GptMessage>, params: &Vec<String>) -> Result<GptAnswer> {
    let openai_api_key = &env::var("OPENAI_API_KEY").expect("foo");

    let client = reqwest::Client::builder()
        .build().expect("foo");

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().expect("foo"));
    headers.insert("Authorization", format!("Bearer {}", openai_api_key).parse().expect("foo"));

    let mut tools = Vec::new();
    if !params.is_empty() {
        let params_description = params.iter().enumerate()
            .map(|(index, param)| format!("[{}] {}", index, param))
            .collect::<Vec<String>>()
            .join("\n");

        tools.push(json!({
            "type": "function",
            "function": {
                "name": "set_params",
                "description": format!("Set or update parameter by index for:\n{}", params_description),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "to_set": {
                            "type": "array",
                            "description": "params to set or update",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "index": {
                                        "type": "integer",
                                        "description": "Index of the parameter"
                                    },
                                    "value": {
                                        "type": "string",
                                        "description": "Value to set or update at the index"
                                    }
                                }
                            }
                        }
                    },
                    "required": ["to_set"]
                }
            }
        }));
    }

    let data = json!({
        "model": "gpt-4",
        "messages": messages,
        "temperature": 0.7,
        "tools": tools
    });

    let response = client.post("https://api.openai.com/v1/chat/completions")
        .headers(headers)
        .json(&data)
        .send()
        .await.expect("foo");

    let body = response.text().await.expect("foo");

    let gpt_answer = json_to_gpt_answer(&body);

    Ok(gpt_answer)
}

fn json_to_gpt_answer(json_str: &String) -> GptAnswer {
    let v: Value = serde_json::from_str(json_str).unwrap();

    let finish_reason = v["choices"][0]["finish_reason"].as_str().unwrap_or("");

    match finish_reason {
        "tool_calls" => {
            let args = v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap_or("");
            let args = args.replace("\\", "");
            let args = args.trim_matches('"');

            let mut function_args = Vec::new();
            let args_json: Value = serde_json::from_str(args).unwrap();
            for arg in args_json["to_set"].as_array().unwrap() {
                let index = arg["index"].as_i64().unwrap() as i32;
                let value = arg["value"].as_str().unwrap().to_string();
                function_args.push(FunctionArg { index, value });
            }
            GptAnswer::FunctionCallArgs(function_args)
        }
        "stop" => {
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            GptAnswer::Question(content)
        }
        _ => panic!("Unknown finish_reason"),
    }
}

fn establish_connection() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(DB_NAME)?;
    Ok(conn)
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    dotenv().ok();

    let conn = establish_connection()?;

    if let Err(err) = prepare_db(&conn).await {
        eprintln!("Failed to prepare_db: {}", err);
        return Err(err);
    }

    if let Err(err) = run_bot().await {
        eprintln!("Failed bot running: {}", err);
        return Err(err);
    }

    Ok(())
}
