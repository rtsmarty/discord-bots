use discord_bots::{discord, error};
use structopt::StructOpt;

use regex::bytes::{
    Regex,
    RegexBuilder,
};
use std::{
    fs::{
        self,
        File,
    },
    io::{
        self,
        Read,
    },
    path::PathBuf,
    rc::Rc,
    time::SystemTime,
};

#[derive(StructOpt)]
struct BotOptions {
    #[structopt(short="t", long="token")]
    token: String,
    #[structopt(short="m", long="mention-file")]
    mention_file: PathBuf,
}

struct Mentions {
    mentions_file: PathBuf,
    last_modified: SystemTime,
    regex_map: Vec<(Regex, Rc<str>)>,
}
impl Mentions {
    fn new(path: PathBuf) -> io::Result<Self> {
        let mut file = File::open(&path)?;
        let mut cfg_file = String::new();
        file.read_to_string(&mut cfg_file)?;
        let metadata = file.metadata()?;

        let mut mentions = Vec::new();
        let mut current_emoji = None;
        // Go through all lines in the specified file which aren't comments
        // (lines starting with "# ")
        for cfg_line in cfg_file.split('\n').filter(|s| s.trim().len() != 0 && !s.trim().starts_with("# ")) {
            // lines starting with whitespace are matcher lines, containing a
            // regular expression to match against
            if cfg_line.starts_with(' ') || cfg_line.starts_with('\t') {
                if let Ok(regex) = RegexBuilder::new(cfg_line.trim()).case_insensitive(true).build() {
                    if let Some(emoji) = current_emoji.as_ref() {
                        mentions.push((regex, Rc::clone(emoji)))
                    } else {
                        eprintln!("No emoji found for regex: {}", cfg_line.trim());
                    }
                } else {
                    eprintln!("Invalid regex: {}", cfg_line.trim());
                }
            // lines starting with regular text specify an actual emoji
            // identifier, all lines underneath (until the next emoji line) will
            // correspond to this emoji
            } else {
                current_emoji = Some(Rc::from(cfg_line.trim()));
            }
        }

        Ok(Self {
            mentions_file: path,
            last_modified: metadata.modified()?,
            regex_map: mentions,
        })
    }
    // If the file has changed since we last checked it, try to overwrite our
    // current mappings with the new ones
    //
    // Ignore any errors, better to have mappings than to try to use a broken
    // file
    fn refresh(&mut self) {
        fs::metadata(&self.mentions_file).ok()
            .and_then(|md| md.modified().ok())
            .and_then(|modified| {
                if self.last_modified < modified {
                    Self::new(self.mentions_file.clone()).ok()
                } else {
                    None
                }
            })
            .map(|val| *self = val);
    }
    // Find the first emoji with a match in the specified emoji file
    fn first_match(&self, bytes: &[u8]) -> Option<Rc<str>> {
        self.regex_map.iter().find(|r| r.0.is_match(bytes)).map(|r| Rc::clone(&r.1))
    }
}


#[tokio::main]
async fn main() -> Result<(), error::Error> {
    let options = BotOptions::from_args();
    let intents = discord::Intents::GUILD_MESSAGES | discord::Intents::DIRECT_MESSAGES;

    let mut mentions = Mentions::new(options.mention_file)?;
    let mut discord = discord::Discord::connect_bot(&options.token, Some(intents)).await?;
    loop {
        match discord.next().await {
            Ok(msg) => {
                let cid = msg.channel_id();
                let mid = msg.message_id();
                mentions.refresh();
                if let Some(r) = mentions.first_match(msg.message().as_bytes()) {
                    tokio::spawn(discord.add_reaction(cid, mid, &r));
                }
            }
            Err(e) => {
                eprintln!("ERROR: {}", e);
                discord = self::discord::Discord::connect_bot(&options.token, Some(intents)).await?;
            }
        }
    }
}
