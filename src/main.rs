mod build_res;
mod common;

use common::ConnectionDetails;
use config;
use matrix_bot_api::handlers::{HandleResult, StatelessHandler};
use matrix_bot_api::{ActiveBot, MatrixBot, Message, MessageType};

use lapin::{Connection, ConnectionProperties};

fn general_help_func(bot: &ActiveBot, message: &Message, cmd: &str) -> HandleResult {
    let cmd_split: Vec<&str> = cmd.split_whitespace().collect();
    match cmd_split.len() {
        0 => {
            bot.send_message(&general_help_str(), &message.room, MessageType::RoomNotice);
        }
        1 => {
            // return HandleResult::ContinueHandling;
            match cmd_split[0] {
                // k if k == "roll" => {
                //     bot.send_message(&dice::help_str(), &message.room, MessageType::RoomNotice)
                // }
                // k if k == "stash" => {
                //     bot.send_message(&stash::help_str(), &message.room, MessageType::RoomNotice)
                // }
                _ => bot.send_message(
                    "Sorry, unknown command",
                    &message.room,
                    MessageType::RoomNotice,
                ),
            }
        }
        _ => {
            bot.send_message("Sorry, that is not possible. Please use \"!help\" or \"!help COMMAND\" for more information.", &message.room, MessageType::RoomNotice);
        }
    };
    HandleResult::StopHandling
}

fn general_help_str() -> String {
    let mut message = "Hi, I'm a friendly robot and provide these options:".to_string();
    message += "\n";
    message += "!help         - Print this help";
    message += "\n";
    message += "!help COMMAND - Print add. help for one of the commands below";
    message += "\n";
    // message += &dice::help_str_short();
    message
}

const SUSE_CONNECTION: ConnectionDetails = ConnectionDetails {
    domain: "suse.de",
    login: "suse:suse",
    buildprefix: "build",
    rabbitprefix: "rabbit",
    rabbitscope: "suse",
};

const OPENSUSE_CONNECTION: ConnectionDetails = ConnectionDetails {
    domain: "opensuse.org",
    login: "opensuse:opensuse",
    buildprefix: "build",
    rabbitprefix: "rabbit",
    rabbitscope: "opensuse",
};

fn main() {
    // ================== Loading credentials ==================
    let mut settings = config::Config::default();
    settings
        .merge(config::File::with_name("botconfig"))
        .unwrap();

    let user = settings.get_str("user").unwrap();
    let password = settings.get_str("password").unwrap();
    let homeserver_url = settings.get_str("homeserver_url").unwrap();
    // =========================================================

    // Defining Prefix - default: "!"
    let prefix = Some(""); // No special prefix at the moment. Replace by Some("myprefix")

    // Defining the first handler for general help
    let mut handler = StatelessHandler::new();
    match prefix {
        Some(x) => handler.set_cmd_prefix(x),
        None => { /* Nothing */ }
    }
    handler.register_handle("help", general_help_func);

    // Creating the bot
    let mut bot = MatrixBot::new(handler);

    for details in [OPENSUSE_CONNECTION, SUSE_CONNECTION].iter() {
        let addr = format!(
            "amqps://{login}@{prefix}.{domain}/%2f",
            login = details.login,
            prefix = details.rabbitprefix,
            domain = details.domain
        );
        let conn = Connection::connect(&addr, ConnectionProperties::default())
            .wait()
            .expect("connection error");

        println!("CONNECTED TO {}", &addr);

        let channel = conn.create_channel().wait().expect("create_channel");

        build_res::subscribe(&mut bot, details, channel);
    }

    bot.run(&user, &password, &homeserver_url);
}
