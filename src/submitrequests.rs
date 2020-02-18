use crate::common::ConnectionDetails;
use anyhow::{anyhow, Result};
use lapin::{
    message::{Delivery, DeliveryResult},
    options::*,
    types::FieldTable,
    Channel, ConsumerDelegate, ExchangeKind,
};
use matrix_bot_api::handlers::{HandleResult, MessageHandler};
use matrix_bot_api::{ActiveBot, MatrixBot, Message, MessageType};
use serde::Deserialize;
use serde_json;
use std::collections::hash_map::HashMap;
use std::sync::{Arc, Mutex};

const KEY_REQUEST_CHANGE: &str = "obs.request.change";
const KEY_REQUEST_STATECHANGE: &str = "obs.request.state_change";
const KEY_REQUEST_DELETE: &str = "obs.request.delete";

#[derive(Deserialize, Debug)]
struct SubmitRequestInfo {
    state: String,
    number: i32,
    author: Option<String>,
    comment: Option<String>,
    description: Option<String>,
    actions: Option<serde_json::Value>,
    when: Option<String>,
    who: Option<String>,
    oldstate: Option<String>,
}

#[derive(Clone)]
struct Subscriber {
    server_details: ConnectionDetails,
    channel: Channel,
    bot: Arc<Mutex<ActiveBot>>,
    subscriptions: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl MessageHandler for Subscriber {
    /// Will be called for every text message send to a room the bot is in
    fn handle_message(&mut self, bot: &ActiveBot, message: &Message) -> HandleResult {
        // Check if its for me
        if !message
            .body
            .contains(&format!("{}/request/", self.server_details.domain))
        {
            return HandleResult::ContinueHandling;
        }

        let parts: Vec<_> = message.body.split("/").collect();
        if parts.len() < 3 {
            println!("Message not parsable");
            bot.send_message(
                "Sorry, I could not parse that. Please post a submitrequest URL",
                &message.room,
                MessageType::TextMessage,
            );
            return HandleResult::ContinueHandling;
        }
        let mut iter = parts.iter().rev();
        // These unwraps cannot fail, as there have to be at least 2 parts
        let number = iter.next().unwrap().trim().to_string();

        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            let key = number;
            if !subscriptions.contains_key(&key) {
                subscriptions.insert(key.clone(), Vec::new());
            }
            subscriptions
                .get_mut(&key)
                .unwrap() // We know its in there, we just added it above
                .push(message.room.to_string());
            println!(
                "Subscribing room {} to {:?} on {}",
                message.room, key, &self.server_details.domain
            );
        } else {
            println!("subscriptions not lockable");
            bot.send_message(
                "Sorry, I could not add your request to the subscriptions, due to an internal error.",
                &message.room,
                MessageType::TextMessage,
            );
        }
        HandleResult::ContinueHandling
    }
}

impl Subscriber {
    fn delivery_wrapper(&self, delivery: Delivery) -> Result<()> {
        let data = std::str::from_utf8(&delivery.data)?;
        let jsondata: SubmitRequestInfo = serde_json::from_str(data)?;
        let changetype;
        if delivery.routing_key.as_str().contains(KEY_REQUEST_CHANGE) {
            changetype = "changed by admin";
        } else if delivery
            .routing_key
            .as_str()
            .contains(KEY_REQUEST_STATECHANGE)
        {
            changetype = "changed";
        } else if delivery.routing_key.as_str().contains(KEY_REQUEST_DELETE) {
            changetype = "deleted";
        } else {
            return Err(anyhow!(
                "Changetype of SR event unknown: {}",
                delivery.routing_key.as_str()
            ));
        }

        let key = format!("{}", jsondata.number);

        let rooms;
        if let Ok(subscriptions) = self.subscriptions.lock() {
            // This is a message we are not subscribed to
            if !subscriptions.contains_key(&key) {
                return Ok(());
            }

            rooms = subscriptions[&key].clone();
        } else {
            return Ok(());
        }

        println!("Request got {}: {}", changetype, jsondata.number);

        if let Ok(bot) = self.bot.lock() {
            for room in rooms {
                bot.send_html_message(
                    &format!(
                        "Request {} was {}: {} ({})",
                        jsondata.number,
                        changetype,
                        jsondata.state,
                        jsondata.comment.as_ref().unwrap_or(&String::new())
                    ),
                    &format!(
                        "<a href={}>Request {}</a> was {}: <strong>{}</strong> ({})",
                        format!(
                            "https://{}.{}/request/show/{}",
                            self.server_details.buildprefix,
                            self.server_details.domain,
                            jsondata.number,
                        ),
                        jsondata.number,
                        changetype,
                        jsondata.state,
                        jsondata.comment.as_ref().unwrap_or(&String::new())
                    ),
                    &room,
                    MessageType::TextMessage,
                );
            }
        }

        self.channel
            .basic_ack(delivery.delivery_tag, BasicAckOptions::default())
            .wait()?;

        Ok(())
    }
}

impl ConsumerDelegate for Subscriber {
    fn on_new_delivery(&self, delivery: DeliveryResult) {
        if let Ok(Some(delivery)) = delivery {
            match self.delivery_wrapper(delivery) {
                Ok(_) => {}
                Err(x) => println!("Error while getting Event: {:?}. Skipping to continue", x),
            }
        } else {
            println!("Delivery not ok");
        }
    }
}

pub fn subscribe(bot: &mut MatrixBot, details: &ConnectionDetails, channel: Channel) -> Result<()> {
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
        .wait()?;

    let queue = channel
        .queue_declare("", QueueDeclareOptions::default(), FieldTable::default())
        .wait()?;

    for key in [
        KEY_REQUEST_CHANGE,
        KEY_REQUEST_STATECHANGE,
        KEY_REQUEST_DELETE,
    ]
    .iter()
    {
        channel
            .queue_bind(
                &queue.name().to_string(),
                "pubsub",
                &format!("{}.{}", details.rabbitscope, key),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .wait()?;
    }

    let consumer = channel
        .basic_consume(
            &queue,
            "OBS_bot_consumer",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .wait()?;

    println!("Subscribing to {}", details.domain);

    let sub = Subscriber {
        server_details: details.clone(),
        channel: channel,
        bot: Arc::new(Mutex::new(bot.get_activebot_clone())),
        subscriptions: Arc::new(Mutex::new(HashMap::new())),
    };
    bot.add_handler(sub.clone());
    consumer.set_delegate(Box::new(sub));

    Ok(())
}