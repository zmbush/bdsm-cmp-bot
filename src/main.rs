use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

const MATCH_URL: &str = "https://bdsmtest.org/ajax/match";
const REGISTRY: &str = "registry.toml";

#[derive(Debug, Deserialize)]
struct MatchResult {
    score: u32,
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

#[derive(Debug, Serialize, Deserialize)]
struct GuildData {
    guild_id: u64,

    person: BTreeMap<serenity::UserId, UserData>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct GlobalData {
    guild: Vec<GuildData>,
}

type Context<'a> = poise::Context<'a, RwLock<GlobalData>, anyhow::Error>;
#[poise::command(slash_command)]
async fn add_bdsm_result(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
    #[description = "The result ID from bdsmtest.org"]
    #[rest]
    id: String,
) -> Result<(), anyhow::Error> {
    let location_id = ctx
        .guild_id()
        .map(|g| g.get())
        .unwrap_or(ctx.channel_id().get());
    let mut data = ctx.data().write().await;

    {
        let guild_index = data
            .guild
            .iter()
            .position(|guild| guild.guild_id == location_id)
            .unwrap_or_else(|| {
                data.guild.push(GuildData {
                    guild_id: location_id,
                    person: BTreeMap::new(),
                });
                data.guild.len() - 1
            });
        let guild = data.guild.get_mut(guild_index).unwrap();
        let person_data = guild
            .person
            .entry(ctx.author().id) //.get().to_string())
            .or_insert_with(UserData::default);
        let headmate_index = person_data
            .0
            .iter()
            .position(|h| h.name == headmate)
            .unwrap_or_else(|| {
                person_data.0.push(HeadmateData {
                    name: headmate,
                    results: BTreeMap::new(),
                });
                person_data.0.len() - 1
            });
        let headmate_data = person_data.0.get_mut(headmate_index).unwrap();
        headmate_data.results.insert(Utc::now(), id);
    }

    let mut output = std::fs::File::create(REGISTRY).context("while opening data file")?;
    write!(
        output,
        "{}",
        toml::to_string_pretty(&*data).context("While formatting toml")?
    )
    .context("While writing data to disk")?;

    ctx.reply("Result Saved")
        .await
        .context("while sending reply")?;

    Ok(())
}

#[poise::command(slash_command)]
/// List the compatibility of yourself and everyone else.
async fn list_compatibility(
    ctx: Context<'_>,
    #[description = "Headmate Name"] headmate: Option<String>,
) -> Result<(), anyhow::Error> {
    let location_id = ctx
        .guild_id()
        .map(|g| g.get())
        .unwrap_or(ctx.channel_id().get());
    let mut data = ctx.data().read().await;
    let guild = data
        .guild
        .iter()
        .find(|g| g.guild_id == location_id)
        .ok_or_else(|| {
            anyhow::anyhow!("No data registered for this guild, use add_bdsm_result first")
        })?;

    let person = guild.person.get(&ctx.author().id).ok_or_else(|| {
        anyhow::anyhow!("You have not registered any results. Use add_bdsm_result first")
    })?;
    let headmate_data = person
        .0
        .iter()
        .find(|h| h.name == headmate)
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
    let client = reqwest::Client::new();
    for (&user_id, person) in &guild.person {
        if user_id == ctx.author().id {
            continue;
        }
        let member_name = match ctx.guild_id() {
            Some(g) => match g.member(ctx, user_id).await {
                Ok(user) => user.display_name().to_string(),
                Err(_) if user_id.get() == 1 => "".to_string(),
                Err(_) => format!("Old user: {user_id}"),
            },
            None => return Err(anyhow::anyhow!("Not in a guild")),
        };

        for headmate in &person.0 {
            let name = headmate.name.as_ref().unwrap_or(&member_name);

            let resp = client
                .post(MATCH_URL)
                .form(&MatchRequest {
                    person: most_recent.clone(),
                    partner: headmate
                        .results
                        .iter()
                        .max_by_key(|h| h.0)
                        .expect("no partner result")
                        .1
                        .clone(),
                })
                .send()
                .await?
                .json::<MatchResult>()
                .await?;
            response += &format!("  x {name}: {:02}%\n", resp.score);
        }
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
            commands: vec![add_bdsm_result(), list_compatibility()],
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

    // let results: TestResult = toml::from_str(&std::fs::read_to_string("registry.toml")?)?;
    // println!("{:?}", results);
    // let client = reqwest::Client::new();

    // for (name, data) in &results.person {
    //     let person = data.values().next().unwrap();
    //     for (partner, data) in &results.person {
    //         let resp = client
    //             .post(MATCH_URL)
    //             .form(&MatchRequest {
    //                 person: person.clone(),
    //                 partner: data.values().next().unwrap().clone(),
    //             })
    //             .send()
    //             .await?
    //             .json::<MatchResult>()
    //             .await?;
    //         println!("{name} x {partner}: {}", resp.score);
    //     }
    // }

    Ok(())
}
