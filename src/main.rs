#![deny(unused)]

use std::collections::{BTreeMap, HashMap};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{self as serenity, Mentionable};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

const MATCH_URL: &str = "https://bdsmtest.org/ajax/match";
const REGISTRY: &str = "registry.json";
const REGISTRY_BKU: &str = "registry.bku.json";

#[derive(Debug, Deserialize)]
struct MatchResult {
    score: u32,
    #[allow(unused)]
    partner: String,
}

#[derive(Clone, Debug, Serialize)]
struct MatchRequest {
    #[serde(rename = "rauth[rid]")]
    person: String,
    partner: String,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
struct HeadmateData {
    results: BTreeMap<DateTime<Utc>, String>,
}

impl HeadmateData {
    fn migrate(&mut self) {}
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
struct UserData {
    #[serde(skip_serializing_if = "Option::is_none")]
    primary: Option<HeadmateData>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headmates: BTreeMap<String, HeadmateData>,
}

impl UserData {
    fn migrate(&mut self) {
        self.primary.iter_mut().for_each(HeadmateData::migrate);
        self.headmates.values_mut().for_each(HeadmateData::migrate)
    }

    pub fn headmate(&self, name: &Option<String>) -> Option<&HeadmateData> {
        match name {
            Some(name) => self.headmates.get(name),
            None => self.primary.as_ref(),
        }
    }

    pub fn headmate_mut(&mut self, name: &Option<String>) -> &mut HeadmateData {
        match name {
            Some(name) => self.headmates.entry(name.clone()).or_default(),
            None => self.primary.get_or_insert_with(HeadmateData::default),
        }
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct GuildData {
    users: BTreeMap<serenity::UserId, UserData>,
}

impl GuildData {
    fn migrate(&mut self) {
        self.users.values_mut().for_each(UserData::migrate)
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct GlobalData {
    guilds: BTreeMap<serenity::GuildId, GuildData>,
}

impl GlobalData {
    fn migrate(&mut self) {
        self.guilds.values_mut().for_each(GuildData::migrate);
    }

    pub fn guild(&self, id: serenity::GuildId) -> Option<&GuildData> {
        self.guilds.get(&id)
    }

    pub fn guild_mut(&mut self, guild_id: serenity::GuildId) -> &mut GuildData {
        self.guilds.entry(guild_id).or_default()
    }
}

fn persist(data: &GlobalData) -> Result<(), anyhow::Error> {
    let _ = std::fs::copy(REGISTRY, REGISTRY_BKU);
    let mut output = std::fs::File::create(REGISTRY).context("while opening data file")?;
    serde_json::to_writer_pretty(&mut output, data).context("while formatting json")?;

    Ok(())
}

async fn get_match(cache: &mut Cache, request: MatchRequest) -> Result<u32, anyhow::Error> {
    let cache_key = Matchup::from(request.clone());
    if let Some(score) = cache.0.get(&cache_key) {
        Ok(*score)
    } else {
        let client = reqwest::Client::new();
        let score = client
            .post(MATCH_URL)
            .form(&request)
            .send()
            .await?
            .json::<MatchResult>()
            .await?
            .score;
        cache.0.insert(cache_key, score);
        Ok(score)
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct Matchup(String, String);

impl Matchup {
    fn new(a: String, b: String) -> Matchup {
        if a < b {
            Matchup(a, b)
        } else {
            Matchup(b, a)
        }
    }
}
impl From<MatchRequest> for Matchup {
    fn from(value: MatchRequest) -> Self {
        Matchup::new(value.person, value.partner)
    }
}

#[derive(Default)]
struct Cache(HashMap<Matchup, u32>);

impl Cache {
    fn new() -> Self {
        Cache::default()
    }
}

struct GlobalState {
    data: RwLock<GlobalData>,
    cache: Mutex<Cache>,
}

type Context<'a> = poise::Context<'a, GlobalState, anyhow::Error>;

#[poise::command(slash_command)]
/// Adds a result from bdsmtest.org. A headmate can also be provided if they took the test on their own.
async fn add_bdsm_result(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
    #[description = "The result ID from bdsmtest.org"]
    #[rest]
    id: String,
) -> Result<(), anyhow::Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let mut data = ctx.data().data.write().await;

    {
        let guild = data.guild_mut(guild_id);
        let person_data = guild
            .users
            .entry(ctx.author().id)
            .or_insert_with(UserData::default);
        let headmate_data = person_data.headmate_mut(&headmate);
        headmate_data.results.insert(Utc::now(), id);
    }

    persist(&data)?;

    ctx.reply("Result Saved")
        .await
        .context("while sending reply")?;

    Ok(())
}

#[poise::command(slash_command)]
/// Removes the entries for the current user (or one of their headmates)
async fn remove_bdsm_results(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
) -> Result<(), anyhow::Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let mut data = ctx.data().data.write().await;

    {
        let guild = data.guild_mut(guild_id);
        let person_data = guild
            .users
            .entry(ctx.author().id)
            .or_insert_with(UserData::default);
        match headmate {
            Some(headmate) => {
                person_data
                    .headmates
                    .remove(&headmate)
                    .ok_or_else(|| anyhow::anyhow!("No entries found for ({headmate})"))?;
            }
            None => {
                person_data
                    .primary
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("No data for primary entry"))?;
            }
        }
    }

    persist(&data)?;

    ctx.reply("Entries Removed")
        .await
        .context("while sending reply")?;

    Ok(())
}

#[poise::command(slash_command)]
/// List the compatibility of yourself and everyone else (including headmates).
async fn list_compatibility(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
) -> Result<(), anyhow::Error> {
    ctx.defer().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let data = ctx.data().data.read().await;
    let guild = data.guild(guild_id).ok_or_else(|| {
        anyhow::anyhow!("No data registered for this guild, use add_bdsm_result first")
    })?;

    let person = guild.users.get(&ctx.author().id).ok_or_else(|| {
        anyhow::anyhow!("You have not registered any results. Use add_bdsm_result first")
    })?;
    let headmate_data = person
        .headmate(&headmate)
        .ok_or_else(|| anyhow::anyhow!("Could not find headmate {headmate:?}"))?;
    let most_recent = headmate_data
        .results
        .iter()
        .max_by_key(|h| h.0)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No results registered for the given headmate. Use add_bdsm_result first"
            )
        })?
        .1;
    let mut response = format!(
        "Compatibility for: {}\n",
        headmate
            .clone()
            .unwrap_or_else(|| match &ctx.author().member {
                Some(m) => serenity::Member::from(serenity::PartialMember::clone(m.as_ref()))
                    .display_name()
                    .to_string(),
                None => ctx
                    .author()
                    .global_name
                    .clone()
                    .unwrap_or(ctx.author().name.clone()),
            })
    );
    let mut results = Vec::new();
    for (&user_id, person) in &guild.users {
        ctx.defer().await?;
        // if user_id == ctx.author().id {
        //     continue;
        // }
        let member_name = match guild_id.member(ctx, user_id).await {
            Ok(user) => user.mention().to_string(),
            // user.display_name().to_string(),
            Err(_) if user_id.get() == 1 => "".to_string(),
            Err(_) => "Deleted User".to_string(),
        };

        if let Some(primary) = &person.primary {
            let score = get_match(
                &mut *ctx.data().cache.lock().await,
                MatchRequest {
                    person: most_recent.clone(),
                    partner: primary
                        .results
                        .iter()
                        .max_by_key(|h| h.0)
                        .expect("no partner result")
                        .1
                        .clone(),
                },
            )
            .await
            .map(|score| score as i32)
            .unwrap_or_else(|_| -1);
            results.push((score, member_name.to_string()));
        }

        for (headmate_name, headmate) in &person.headmates {
            let name = format!("{member_name} ({headmate_name})",);
            let score = get_match(
                &mut *ctx.data().cache.lock().await,
                MatchRequest {
                    person: most_recent.clone(),
                    partner: headmate
                        .results
                        .iter()
                        .max_by_key(|h| h.0)
                        .expect("no partner result")
                        .1
                        .clone(),
                },
            )
            .await
            .map(|score| score as i32)
            .unwrap_or_else(|_| -1);
            results.push((score, name.to_string()));
        }
    }

    results.sort_by_key(|(s, _)| -s);
    for (score, name) in results {
        response += &format!(
            "- {name}: {}\n",
            if score >= 0 {
                format!("{score:02}%")
            } else {
                "Invalid Result".to_string()
            }
        );
    }

    ctx.send(
        poise::CreateReply::default()
            .content(response)
            .reply(true)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    dotenv::dotenv()?;

    let token = std::env::var("DISCORD_TOKEN")?;
    let intents = serenity::GatewayIntents::non_privileged();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                add_bdsm_result(),
                list_compatibility(),
                remove_bdsm_results(),
            ],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                let mut results: GlobalData =
                    serde_json::from_str(&std::fs::read_to_string(REGISTRY).unwrap_or_default())?;
                results.migrate();
                let _ = persist(&results);
                Ok(GlobalState {
                    data: RwLock::new(results),
                    cache: Mutex::new(Cache::new()),
                })
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client.unwrap().start().await.unwrap();

    Ok(())
}
