// $env:RUST_LOG="debug"; $env:TELOXIDE_TOKEN=""; cargo run
use std::collections::HashMap;
use std::{env, fs};
use std::fs::File;

use chrono::{Datelike, Timelike, Utc};
use csv::WriterBuilder;
use dotenv::dotenv;
use env_logger;
use log::*;
use reqwest::header::HeaderMap;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, InputFile};


const DB_NAME: &str = "db.sqlite";
const USER_TABLE_NAME: &str = "user";
const SURVEY_TABLE_NAME: &str = "survey";
const DEFAULT_PARAMS: [&str; 2] = ["Name", "Phone"];
const DEFAULT_PROMPT: &str = "# Character\nYou are an HR specialist at Google Meta. Your task is to interview candidates for the position of Marketing Manager.\n\n## Skills\n\n### Skill 1: Gathering Candidate Information\n- Inquire about the following:\n{params}\n\n## Constraints:\n- Ensure the conversation continues until all information is gathered. Once complete, bid farewell and inform the candidate that they will receive a response regarding their candidacy via the provided email address or phone number. Provide these contact details in your closing statement. No need to ask more if there is nothing in \"Inquire about the following\". If person ask to change data, do it.";
const PROMPT_PARAMS_TEMPLATE: &str = "{params}";
const HISTORY_LIMIT: usize = 10000;
const MAX_USER_TOKENS: i64 = 5000;

fn create_tables(conn: &Connection) {
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS {} (
            id INTEGER PRIMARY KEY,
            data TEXT NOT NULL
        )", SURVEY_TABLE_NAME),
        [],
    ).expect("foo");

    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS {} (
            id INTEGER PRIMARY KEY,
            chat_id INTEGER NOT NULL,
            tokens INTEGER NOT NUll,
            data TEXT NOT NULL
        )", USER_TABLE_NAME),
        [],
    ).expect("foo");
}

fn change_or_create_survey(survey_config: &SurveyConfig, conn: &Connection) {
    let survey_config_json = serde_json::to_string(&survey_config);

    let survey_config_str = match survey_config_json {
        Ok(value) => value,
        Err(_) => { panic!("Invalid survey_config_data") }
    };

    let existing_survey: Option<i64> = match conn.query_row(
        &format!("SELECT id FROM {} ORDER BY id LIMIT 1", SURVEY_TABLE_NAME),
        [],
        |row| row.get(0),
    ) {
        Ok(id) => Some(id),
        Err(_) => None,
    };

    if let Some(survey_id) = existing_survey {
        conn.execute(
            &format!("UPDATE {} SET data = ? WHERE id = ?", SURVEY_TABLE_NAME),
            params![survey_config_str, survey_id],
        ).expect("foo");
        info!("Survey updated successfully");
    } else {
        conn.execute(
            &format!("INSERT INTO {} (data) VALUES (?)", SURVEY_TABLE_NAME),
            params![survey_config_str],
        ).expect("foo");
        info!("New survey created successfully");
    }
}

fn get_survey_data(conn: &Connection) -> Result<String, rusqlite::Error> {
    let data: String = conn.query_row(
        &format!("SELECT data FROM {} ORDER BY id LIMIT 1", SURVEY_TABLE_NAME),
        [],
        |row| row.get(0),
    )?;
    Ok(data)
}

fn get_user_data_json(chat_id: i64, conn: &Connection) -> Result<(i64, String), rusqlite::Error> {
    let row = conn.query_row(
        &format!("SELECT tokens, data FROM {} WHERE chat_id = ? ORDER BY id LIMIT 1", USER_TABLE_NAME),
        params![chat_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(row)
}

fn clear_user_table(conn: &Connection) {
    conn.execute(
        &format!("DELETE FROM {}", USER_TABLE_NAME),
        [],
    ).expect("foo");
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

    fn format_prompt(&self, unknown_str: &str) -> String {
        let mut prompt = self.prompt.clone();

        if let Some(index) = prompt.find(PROMPT_PARAMS_TEMPLATE) {
            prompt.replace_range(index..(index + PROMPT_PARAMS_TEMPLATE.len()), unknown_str);
        }

        prompt
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
) -> (Option<(i64, UserData)>, bool) {
    match get_user_data_json(chat_id, conn) {
        Ok((tokens, json_string)) => {
            if let Some(user_data) = extract_user_data(&json_string) {
                (Some((tokens, user_data)), false)
            } else if or_default {
                (Some((MAX_USER_TOKENS, UserData::default(params))), true)
            } else {
                (None, false)
            }
        }
        Err(_) if or_default => (Some((MAX_USER_TOKENS, UserData::default(params))), true),
        _ => (None, false),
    }
}

fn create_default_survey(conn: &Connection) {
    let survey_config = get_default_survey_config();

    change_or_create_survey(&survey_config, &conn);
}

fn get_default_survey_config() -> SurveyConfig {
    return SurveyConfig::new(
        DEFAULT_PARAMS.iter().map(|&s| s.into()).collect(),
        DEFAULT_PROMPT.to_string(),
    );
}

fn prepare_db(conn: &Connection) {
    create_tables(&conn);

    match get_survey_config(&conn) {
        Some(survey_config) => {
            info!("Existing survey config - {:?}", survey_config);
        }
        _ => {
            create_default_survey(&conn);
            info!("Default survey config created successfully");
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Param {
    index: u16,
    name: String,
    value: Option<String>,
}


#[derive(Clone, Debug, Serialize, Deserialize)]
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
    tokens: i64,
    survey_config: SurveyConfig,
    _data: UserData,
}

impl UserSurvey {
    fn new(chat_id: i64, survey_config: SurveyConfig, conn: &Connection) -> Self {
        let (tokens, _data) = get_user_data(chat_id, &survey_config.params, conn, true).0.expect("foo");
        Self { chat_id, tokens, survey_config, _data }
    }

    fn set_param(&mut self, arg: &FunctionArg) -> Result<String> {
        if arg.index < 0 || arg.index as usize >= self._data.params.len() {
            panic!("foo")
        }

        let param = &mut self._data.params[arg.index as usize];
        param.value = Some(arg.value.clone());

        // TODO not changing?

        Ok(param.name.to_string())
    }

    fn get_gpt_messages(&mut self) -> Vec<GptMessage> {
        let mut known: HashMap<usize, String> = HashMap::new();
        for param in &self._data.params {
            if let Some(value) = &param.value {
                known.insert(param.index as usize, value.clone());
            }
        }

        let mut know_unknown = String::new();
        for (index, name) in self.survey_config.params.iter().enumerate() {
            if let Some(value) = known.get(&index) {
                know_unknown.push_str(&format!("[{}] - {} - {} (already set)\n", index, name, value));
            } else {
                know_unknown.push_str(&format!("[{}] - {} - ? (need to ask)\n", index, name));
            }
        }

        let mut result = vec![
            GptMessage {
                role: "system".to_string(),
                content: self.survey_config.format_prompt(&know_unknown),
            }
        ];
        let mut chars_count = result[0].content.len();

        for msg in self._data.messages.iter().rev() {
            chars_count += msg.content.len();
            if chars_count > HISTORY_LIMIT {
                break;
            }
            result.insert(1, msg.clone());
        }

        result
    }

    fn add_user_answer(&mut self, text: &str) {
        self.add_message("user", text);
    }

    fn add_system_text(&mut self, text: &str) {
        self.add_message("system", text);
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

        let existing_data: Option<i32> = match conn.query_row(
            &format!("SELECT id FROM {} WHERE chat_id = ?", USER_TABLE_NAME),
            params![self.chat_id],
            |row| row.get(0),
        ) {
            Ok(id) => Some(id),
            Err(_) => None,
        };

        match existing_data {
            Some(_) => {
                conn.execute(
                    &format!(
                        "UPDATE {} SET data = ?, tokens = ? WHERE chat_id = ?",
                        USER_TABLE_NAME
                    ),
                    params![data_str, self.tokens, self.chat_id],
                )?;
            }
            None => {
                conn.execute(
                    &format!(
                        "INSERT INTO {} (chat_id, tokens, data) VALUES (?, ?, ?)",
                        USER_TABLE_NAME
                    ),
                    params![self.chat_id, self.tokens, data_str],
                ).expect("Insertion failed");
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
    bot.send_chat_action(msg.chat.id, ChatAction::Typing).await?;

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
        clear_user_table(&conn);
        bot.send_message(msg.chat.id, "User table cleared").await?;
        return Ok(());
    }

    if text == "/export_csv" {
        let (file_path, document) = get_document(&user_survey, &conn).expect("foo");
        bot.send_document(msg.chat.id, document).await?;
        delete_document(&file_path);
        return Ok(());
    }

    if text == "/start" || text == "/help" {
        let help_text = &env::var("HELP_TEXT").unwrap_or("START".to_string());
        bot.send_message(msg.chat.id, help_text).await?;
        return Ok(());
    }

    let prompt_command = "/prompt";
    if text.starts_with(prompt_command) {
        let new_prompt = get_command_body(prompt_command, text);
        let answer;
        if new_prompt.contains(PROMPT_PARAMS_TEMPLATE) {
            user_survey.survey_config.prompt = new_prompt.clone();
            change_or_create_survey(&user_survey.survey_config, &conn);
            clear_user_table(&conn);
            answer = format!("New prompt:\n{}", new_prompt);
        } else {
            answer = format!("Invalid prompt. Prompt must contain \"{}\"", PROMPT_PARAMS_TEMPLATE);
        }
        bot.send_message(msg.chat.id, answer).await?;
        return Ok(());
    }

    let params_command = "/params";
    if text.starts_with(params_command) {
        let new_params_text = get_command_body(params_command, text);
        let new_params: Vec<String> = new_params_text
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();

        let answer;
        if new_params.is_empty() {
            answer = "No valid parameters provided. Please provide valid parameters.".to_string();
        } else {
            user_survey.survey_config.params = new_params.clone();
            change_or_create_survey(&user_survey.survey_config, &conn);
            clear_user_table(&conn);
            answer = format!("Parameters updated:\n{}", new_params.join("\n"));
        }

        bot.send_message(msg.chat.id, answer).await?;
        return Ok(());
    }

    user_survey.add_user_answer(text);
    let (spent_tokens, question) = get_question(&mut user_survey, 1, 0).await.expect("foo").unwrap();

    bot.send_message(msg.chat.id, &question).await?;

    user_survey.tokens -= spent_tokens;
    user_survey.add_assistant_question(question.as_str());
    user_survey.sync_data(&conn).ok();
    // info!("{:?}", user_survey);

    Ok(())
}

fn get_command_body(prefix: &str, text: &str) -> String {
    text.trim_start_matches(prefix).trim().to_string()
}


async fn get_question(user_survey: &mut UserSurvey, counter: u8, tokens: i64) -> Result<Option<(i64, String)>, Box<dyn std::error::Error>> {
    if counter > 3 {
        return Ok(Some((tokens, "Error get question".to_string())));    // TODO - don't save
    }
    if user_survey.tokens < 0 {
        return Ok(Some((tokens, "Have a great day!".to_string())));     // TODO - don't save
    }

    let messages = &user_survey.get_gpt_messages();
    let params = &user_survey.survey_config.params;

    let (gpt_answer, spent_tokens) = get_openai_question(messages, params).await?;

    match gpt_answer {
        GptAnswer::Question(question) => Ok(Some((spent_tokens + tokens, question))),
        GptAnswer::FunctionCallArgs(call_args) => {
            for arg in &call_args {
                let param_name = user_survey.set_param(&arg)?;
                user_survey.add_system_text(&format!("Param [{}] {} set to {}!", arg.index, param_name, arg.value));
            }

            Box::pin(get_question(user_survey, counter + 1, spent_tokens + tokens)).await
        }
    }
}

fn get_document(user_survey: &UserSurvey, conn: &Connection) -> Result<(String, InputFile)> {
    let rows: Result<Vec<String>, rusqlite::Error> = conn
        .prepare(&format!("SELECT data FROM {}", USER_TABLE_NAME))?
        .query_map([], |row| row.get(0))?
        .collect();

    let rows: Vec<String> = rows?;
    let mut user_data_array: Vec<UserData> = vec![];
    for json_string in rows.iter() {
        if let Some(user_data) = extract_user_data(&json_string) {
            user_data_array.push(user_data)
        }
    }

    let mut csv_data: Vec<Vec<&str>> = Vec::new();

    for user_data in user_data_array.iter() {
        let mut row: Vec<&str> = Vec::new();
        for header in &user_survey.survey_config.params {
            if let Some(param) = user_data.params.iter().find(|param| param.name == *header) {
                if let Some(value) = &param.value {
                    row.push(value);
                } else {
                    row.push("");
                }
            } else {
                row.push("");
            }
        }
        csv_data.push(row);
    }

    let dt_now = Utc::now();
    let formatted_date_time = format!(
        "{:04}-{:02}-{:02}T{:02}_{:02}_{:.3}Z",
        dt_now.year(),
        dt_now.month(),
        dt_now.day(),
        dt_now.hour(),
        dt_now.minute(),
        dt_now.second() as f32 + dt_now.nanosecond() as f32 / 1_000_000_000.0
    );
    let file_path = &format!("export_{}.csv", formatted_date_time);

    let file = File::create(file_path).expect("foo");
    let mut csv_writer = WriterBuilder::new()
        .has_headers(false)
        .from_writer(file);

    csv_writer.write_record(&user_survey.survey_config.params).expect("foo");
    for row in csv_data.iter() {
        csv_writer.write_record(row).expect("foo");
    }
    csv_writer.flush().expect("foo");

    Ok((file_path.to_string(), InputFile::file(file_path)))
}

fn delete_document(file_path: &str) {
    fs::remove_file(file_path).expect("foo")
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

async fn get_openai_question(messages: &Vec<GptMessage>, params: &Vec<String>) -> Result<(GptAnswer, i64)> {
    let openai_api_key = &env::var("OPENAI_API_KEY").expect("foo");

    let client = reqwest::Client::builder()
        .build().expect("foo");

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().expect("foo"));
    headers.insert("Authorization", format!("Bearer {}", openai_api_key).parse().expect("foo"));

    let mut tools = Vec::new();
    if !params.is_empty() {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "set_params",
                "description": "Set parameter values by index",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "to_set": {
                            "type": "array",
                            "description": "Array of parameters to set",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "index": {
                                        "type": "integer",
                                        "description": "Index of the parameter"
                                    },
                                    "value": {
                                        "description": "Value to set at the index"
                                    }
                                },
                                "required": ["index", "value"]
                            }
                        }
                    },
                    "required": ["to_set"]
                }
            }
        }));
    }

    let data = json!({
        "model": "gpt-3.5-turbo",
        "messages": messages,
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

fn json_to_gpt_answer(json_str: &String) -> (GptAnswer, i64) {
    let v: Value = serde_json::from_str(json_str).unwrap();

    let finish_reason = v["choices"][0]["finish_reason"].as_str().unwrap_or("");
    let spent_tokens = v["usage"]["total_tokens"].as_i64().unwrap_or(0);

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
            (GptAnswer::FunctionCallArgs(function_args), spent_tokens)
        }
        "stop" => {
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            (GptAnswer::Question(content), spent_tokens)
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

    prepare_db(&conn);

    if let Err(err) = run_bot().await {
        eprintln!("Failed bot running: {}", err);
        return Err(err);
    }

    Ok(())
}
