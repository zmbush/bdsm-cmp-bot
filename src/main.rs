use std::collections::BTreeMap;
use std::io::Write;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const MATCH_URL: &str = "https://bdsmtest.org/ajax/match";
const REGISTRY: &str = "registry.toml";
const REGISTRY_BKU: &str = "registry.bku.toml";

#[derive(Debug, Deserialize)]
struct MatchResult {
    score: u32,
    #[allow(unused)]
    partner: String,
}

#[derive(Debug, Serialize)]
struct MatchRequest {
    #[serde(rename = "rauth[rid]")]
    person: String,
    partner: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TestResult {
    person: BTreeMap<String, BTreeMap<DateTime<Utc>, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadmateData {
    name: Option<String>,
    results: BTreeMap<DateTime<Utc>, String>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct UserData(Vec<HeadmateData>);

impl UserData {
    pub fn headmate(&self, name: &Option<String>) -> Option<&HeadmateData> {
        self.0.iter().find(|h| h.name == *name)
    }

    pub fn headmate_mut(&mut self, name: &Option<String>) -> &mut HeadmateData {
        let headmate_index = self
            .0
            .iter()
            .position(|h| h.name == *name)
            .unwrap_or_else(|| {
                self.0.push(HeadmateData {
                    name: name.clone(),
                    results: BTreeMap::new(),
                });
                self.0.len() - 1
            });
        self.0.get_mut(headmate_index).unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct GuildData {
    guild_id: serenity::GuildId,

    person: BTreeMap<serenity::UserId, UserData>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct GlobalData {
    guild: Vec<GuildData>,
}

impl GlobalData {
    pub fn guild(&self, id: serenity::GuildId) -> Option<&GuildData> {
        self.guild.iter().find(|guild| guild.guild_id == id)
    }

    pub fn guild_mut(&mut self, guild_id: serenity::GuildId) -> &mut GuildData {
        let guild_index = self
            .guild
            .iter()
            .position(|guild| guild.guild_id == guild_id)
            .unwrap_or_else(|| {
                self.guild.push(GuildData {
                    guild_id,
                    person: BTreeMap::new(),
                });
                self.guild.len() - 1
            });
        self.guild.get_mut(guild_index).unwrap()
    }
}

fn persist(data: &GlobalData) -> Result<(), anyhow::Error> {
    let _ = std::fs::copy(REGISTRY, REGISTRY_BKU);
    let mut output = std::fs::File::create(REGISTRY).context("while opening data file")?;
    write!(
        output,
        "{}",
        toml::to_string_pretty(data).context("While formatting toml")?
    )
    .context("While writing data to disk")?;

    Ok(())
}

async fn get_match(request: MatchRequest) -> Result<u32, anyhow::Error> {
    let client = reqwest::Client::new();
    Ok(client
        .post(MATCH_URL)
        .form(&request)
        .send()
        .await?
        .json::<MatchResult>()
        .await?
        .score)
}

type Context<'a> = poise::Context<'a, RwLock<GlobalData>, anyhow::Error>;
#[poise::command(slash_command)]
/// Adds a result from bdsmtest.org. A headmate can also be provided if they took the test on their own.
async fn add_bdsm_result(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
    #[description = "The result ID from bdsmtest.org"]
    #[rest]
    id: String,
) -> Result<(), anyhow::Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let mut data = ctx.data().write().await;

    {
        let guild = data.guild_mut(guild_id);
        let person_data = guild
            .person
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
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let mut data = ctx.data().write().await;

    {
        let guild = data.guild_mut(guild_id);
        let person_data = guild
            .person
            .entry(ctx.author().id)
            .or_insert_with(UserData::default);
        if let Some(headmate_index) = person_data.0.iter().position(|h| h.name == headmate) {
            person_data.0.swap_remove(headmate_index);
        } else {
            return Err(anyhow::anyhow!("No entries found for ({headmate:?})"));
        }
    }

    persist(&data)?;

    ctx.reply("Entries removed")
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
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("No guild id. Must be in a guild"))?;
    let data = ctx.data().read().await;
    let guild = data.guild(guild_id).ok_or_else(|| {
        anyhow::anyhow!("No data registered for this guild, use add_bdsm_result first")
    })?;

    let person = guild.person.get(&ctx.author().id).ok_or_else(|| {
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
    for (&user_id, person) in &guild.person {
        ctx.defer().await?;
        if user_id == ctx.author().id {
            continue;
        }
        let member_name = match ctx.guild_id() {
            Some(g) => match g.member(ctx, user_id).await {
                Ok(user) => user.display_name().to_string(),
                Err(_) if user_id.get() == 1 => "".to_string(),
                Err(_) => "Deleted User".to_string(),
            },
            None => return Err(anyhow::anyhow!("Not in a guild")),
        };

        for headmate in &person.0 {
            let name = format!(
                "{member_name}{}",
                match headmate.name {
                    Some(ref name) => format!(" ({name})"),
                    None => String::new(),
                }
            );
            let score = get_match(MatchRequest {
                person: most_recent.clone(),
                partner: headmate
                    .results
                    .iter()
                    .max_by_key(|h| h.0)
                    .expect("no partner result")
                    .1
                    .clone(),
            })
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

    ctx.reply(response).await?;

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
                let results: GlobalData =
                    toml::from_str(&std::fs::read_to_string(REGISTRY).unwrap_or_default())
                        .unwrap_or_default();
                Ok(RwLock::new(results))
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client.unwrap().start().await.unwrap();

    Ok(())
}
