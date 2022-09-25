#![recursion_limit="512"]
#![feature(hash_set_entry, try_blocks)]

use discord_bots::{discord, chain, error};

use bytes::Bytes;
use clap::Parser;
use futures::{
    pin_mut,
    future::FutureExt,
};
use std::{
    collections::{
        hash_map::HashMap,
        hash_set::HashSet,
    },
    str,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

const MAX_MESSAGE_LENGTH: usize = 2000;

#[derive(Parser)]
struct BotOptions {
    #[clap(short='l', long="chain-len", default_value_t=8)]
    chain_length: usize,
    #[clap(short='t', long="token")]
    token: String,
    #[clap(short='b', long="backlog-len", default_value_t=100)]
    backlog_len: usize,
    #[clap(short='g', long="whole-guild-logs")]
    whole_guild_logs: bool,
}

struct BacklogMessage {
    msg:      discord::Message,
    guild_id: Option<Bytes>
}

async fn get_old_messages(mut messages: discord::ChannelMessages, gid: Option<Bytes>, tx: UnboundedSender<BacklogMessage>) {
    let res: Result<(), error::Error> = try {
        while let Some(msg) = messages.next().await? {
            let guild_id = msg.guild_id_buf().cloned().or_else(|| gid.clone());
            tx.send(BacklogMessage { msg, guild_id }).map_err(|_| error::Error::SendChannelClosed)?;
        }
    };
    if let Err(e) = res {
        eprintln!("Failed to get old message: {}", e);
    }
}


#[tokio::main]
async fn main() -> Result<(), error::Error> {
    let options = BotOptions::from_args();
    let intents =
        discord::Intents::GUILD_MESSAGES | discord::Intents::DIRECT_MESSAGES;

    let mut discord = discord::Discord::connect_bot(&options.token, Some(intents)).await?;
    let mut rng = rand::thread_rng();

    // These all use Bytes as a key, which is a known false positive for this
    // lint
    #[allow(clippy::mutable_key_type)]
    let mut channel_chains = HashMap::new();
    #[allow(clippy::mutable_key_type)]
    let mut guild_chains = HashMap::new();
    #[allow(clippy::mutable_key_type)]
    let mut encountered_channels = HashSet::new();

    let (tx, mut rx) = unbounded_channel::<BacklogMessage>();

    loop {
        let res = {
            let next = discord.next().fuse();
            pin_mut!(next);
            loop {
                // Favour incoming messages over backlog messages
                futures::select_biased! {
                    // We've received a real message, continue
                    msg_res = next => break msg_res,
                    // We've got a backlog message, just feed it to the chain
                    // and continue until we finsih getting our next real
                    // message
                    backlog = rx.recv().fuse() => if let Some(backlog) = backlog {
                        let chain = if let (Some(guild_id_buf), true) = (backlog.guild_id, options.whole_guild_logs) {
                            guild_chains.entry(guild_id_buf)
                                .or_insert_with(|| chain::Chain::new(options.chain_length))
                        } else {
                            channel_chains.entry(backlog.msg.channel_id_buf().clone())
                                .or_insert_with(|| chain::Chain::new(options.chain_length))
                        };
                        if !backlog.msg.is_me() && !backlog.msg.message().is_empty() && !backlog.msg.mentioned() {
                            chain.feed(backlog.msg.message_buf().clone());
                        }
                    } else {
                        return Err(error::Error::SendChannelClosed)
                    }
                }
            }
        };
        match res {
            Ok(msg) => {
                let chain = if let (Some(guild_id_buf), true) = (msg.guild_id_buf(), options.whole_guild_logs) {
                    encountered_channels.get_or_insert_with(msg.channel_id_buf(), |buf| {
                        let old_messages = discord.channel_messages(msg.channel_id(), options.backlog_len, None);
                        tokio::spawn(get_old_messages(old_messages, Some(guild_id_buf.clone()), tx.clone()));
                        buf.clone()
                    });

                    guild_chains.entry(guild_id_buf.clone())
                        .or_insert_with(|| chain::Chain::new(options.chain_length))
                } else {
                    channel_chains.entry(msg.channel_id_buf().clone())
                        .or_insert_with(|| {
                            let old_messages = discord.channel_messages(msg.channel_id(), options.backlog_len, None);
                            tokio::spawn(get_old_messages(old_messages, None, tx.clone()));
                            chain::Chain::new(options.chain_length)
                        })
                };

                if !msg.is_me() && !msg.message().is_empty() {
                    if !msg.mentioned() {
                        chain.feed(msg.message_buf().clone());
                    } else {
                        let mut message = String::new();

                        // The messages we receive should all be UTF-8
                        // (otherwise the Deserialization will fail, the
                        // underlying Discord models assume a str not just
                        // bytes), so this should in theory never fail, but I
                        // don't know enough about UTF-8 or unicode to guarantee
                        // that so I just try 10 times to build a valid string
                        // and if I still can't build a message after than, just
                        // ignore the message
                        for _ in 0..10 {
                            let bytes = chain.generator(&mut rng).take(MAX_MESSAGE_LENGTH.saturating_sub(message.len())).collect::<Vec<_>>();
                            if let Ok(s) = str::from_utf8(&bytes) {
                                message.push_str(s);
                                break;
                            }
                        }
                        if !message.is_empty() {
                            let msg = discord.send_message(msg.channel_id(), &message);
                            tokio::spawn(async move {
                                let res = msg.await;
                                if let Err(e) = res {
                                    eprintln!("Failed to send message: {}", e);
                                }
                            });
                        } else {
                            eprintln!("Failed to build message");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("ERROR: {}", e);
                // Just try to reconnect if we can so that we keep all of the
                // chains we have built rather than killing the process and
                // starting from scratch again
                discord = self::discord::Discord::connect_bot(&options.token, Some(intents)).await?;
            }
        }
    }
}
