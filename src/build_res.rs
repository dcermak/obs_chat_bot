use lapin::{
    message::DeliveryResult, options::*, types::FieldTable, Channel, ConsumerDelegate, ExchangeKind,
};
use matrix_bot_api::handlers::{HandleResult, MessageHandler};
use matrix_bot_api::{ActiveBot, MatrixBot, Message, MessageType};
use serde::Deserialize;
use serde_json;
use std::collections::hash_map::HashMap;

use std::sync::{Arc, Mutex};

const KEY_BUILD_SUCCESS: &str = "suse.obs.package.build_success";
const KEY_BUILD_FAIL: &str = "suse.obs.package.build_fail";

#[derive(Deserialize, Debug)]
struct BuildSuccess {
    arch: String,
    repository: String,
    package: String,
    project: String,
    reason: Option<String>,
    release: Option<String>,
    readytime: Option<String>,
    srcmd5: Option<String>,
    rev: Option<String>,
    bcnt: Option<String>,
    verifymd5: Option<String>,
    starttime: Option<String>,
    endtime: Option<String>,
    workerid: Option<String>,
    versrel: Option<String>,
    hostarch: Option<String>,
    previouslyfailed: Option<String>,
}

#[derive(Clone)]
struct Subscriber {
    channel: Channel,
    bot: Arc<Mutex<ActiveBot>>,
    subscriptions: Arc<Mutex<HashMap<(String, String), Vec<String>>>>,
}

impl MessageHandler for Subscriber {
    /// Will be called for every text message send to a room the bot is in
    fn handle_message(&mut self, bot: &ActiveBot, message: &Message) -> HandleResult {
        let parts: Vec<_> = message.body.split("/").collect();
        println!("Got a message");
        if parts.len() < 2 {
            println!("Message not parsable");
            bot.send_message(
                "Sorry, I could not parse that. Usage: PROJECT/PACKAGE",
                &message.room,
                MessageType::TextMessage,
            );
            return HandleResult::ContinueHandling;
        }
        let mut iter = parts.iter().rev();
        let package = iter.next().unwrap().to_string();
        let project = iter.next().unwrap().to_string();
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            let key = (project.clone(), package.clone());
            if !subscriptions.contains_key(&key) {
                subscriptions.insert(key.clone(), Vec::new());
            }
            subscriptions
                .get_mut(&key)
                .unwrap() // We know its in there, we just added it above
                .push(message.room.to_string());
            println!("Subscribing room {} to {:?}", message.room, key);
        } else {
            println!("subscriptions not lockable");
        }
        HandleResult::ContinueHandling
    }
}

impl ConsumerDelegate for Subscriber {
    fn on_new_delivery(&self, delivery: DeliveryResult) {
        if let Ok(Some(delivery)) = delivery {
            let data = std::str::from_utf8(&delivery.data).unwrap();
            let jsondata: BuildSuccess = serde_json::from_str(data).unwrap();

            let build_res;
            if delivery.routing_key.as_str() == KEY_BUILD_SUCCESS {
                build_res = "success";
            } else if delivery.routing_key.as_str() == KEY_BUILD_FAIL {
                build_res = "failed";
            } else {
                panic!(
                    "Build event neither success nor failure, but {}",
                    delivery.routing_key.as_str()
                );
            }

            let key = (jsondata.project.clone(), jsondata.package.clone());
            let rooms;
            if let Ok(subscriptions) = self.subscriptions.lock() {
                if !subscriptions.contains_key(&key) {
                    return;
                }

                rooms = subscriptions[&key].clone();
            } else {
                return;
            }

            println!(
                "Build {}: {} {} ({})",
                build_res, jsondata.project, jsondata.package, jsondata.arch
            );

            if let Ok(bot) = self.bot.lock() {
                for room in rooms {
                    bot.send_message(
                        &format!(
                            "Build {}: {} {} ({})",
                            build_res, jsondata.project, jsondata.package, jsondata.arch
                        ),
                        &room,
                        MessageType::TextMessage,
                    );
                }
            }

            self.channel
                .basic_ack(delivery.delivery_tag, BasicAckOptions::default())
                .wait()
                .expect("basic_ack");
        } else {
            println!("Delivery not ok");
        }
    }
}

pub fn subscribe(bot: &mut MatrixBot, channel: Channel) {
    channel
        .exchange_declare(
            "pubsub",
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                passive: true,
                durable: true,
                auto_delete: true, // deactivate me to survive bot reboots
                internal: false,
                nowait: false,
            },
            FieldTable::default(),
        )
        .wait()
        .expect("exchange_declare");

    let queue = channel
        .queue_declare("", QueueDeclareOptions::default(), FieldTable::default())
        .wait()
        .expect("queue_declare");

    for key in [KEY_BUILD_SUCCESS, KEY_BUILD_FAIL].iter() {
        channel
            .queue_bind(
                &queue.name().to_string(),
                "pubsub",
                key,
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .wait()
            .expect("queue_bind");
    }

    let consumer = channel
        .basic_consume(
            &queue,
            "OBS_bot_consumer",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .wait()
        .expect("basic_consume");
    let sub = Subscriber {
        channel: channel,
        bot: Arc::new(Mutex::new(bot.get_activebot_clone())),
        subscriptions: Arc::new(Mutex::new(HashMap::new())),
    };
    bot.add_handler(sub.clone());
    consumer.set_delegate(Box::new(sub));
}