mod args;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::*};
use reqwest::Client;
use serde_json::{Value, from_str, json};
use std::{
    collections::HashMap,
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::RwLock, time::sleep};

#[derive(Debug, Clone)]
struct SensorReading {
    psu_pin: Option<u64>,
    cpu_power: Option<u64>,
    cpu0_power: Option<u64>,
    cpu1_power: Option<u64>,
    fan_power: Option<u64>,
    cpu0_temp: Option<u64>,
    cpu1_temp: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = args::Args::parse();
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let tokens = get_tokens(&client, &args.ips).await?;
    let readings = Arc::new(RwLock::new(HashMap::<String, SensorReading>::new()));

    let read_ips = args.ips.clone();
    let client_clone = client.clone();
    let tokens_clone = tokens.clone();
    let readings_clone = Arc::clone(&readings);

    tokio::spawn(async move {
        loop {
            update_readings(&client_clone, &read_ips, &tokens_clone, &readings_clone).await;
            sleep(Duration::from_secs(1)).await;
        }
    });

    start_ui(&args.ips, readings).await
}

async fn get_tokens(client: &Client, ips: &[String]) -> Result<Vec<String>> {
    let login_json = json!({
        "UserName": "admin",
        "Password": "admin"
    });

    let mut tokens = Vec::new();

    for ip in ips {
        let login_url = format!("https://{}/redfish/v1/SessionService/Sessions", ip);
        let resp = client
            .post(&login_url)
            .header("Content-Type", "application/json")
            .json(&login_json)
            .send()
            .await?;

        let text = resp.text().await?;
        let json: Value = from_str(&text)?;
        let token = json["Oem"]["Public"]["X-Auth-Token"]
            .as_str()
            .unwrap_or("")
            .to_string();
        tokens.push(token);
    }

    Ok(tokens)
}

async fn update_readings(
    client: &Client,
    ips: &[String],
    tokens: &[String],
    readings: &Arc<RwLock<HashMap<String, SensorReading>>>,
) {
    let mut map = HashMap::new();

    for (ip, token) in ips.iter().zip(tokens.iter()) {
        let sensor_url = format!("https://{}/redfish/v1/Chassis/1/ThresholdSensors", ip);
        if let Ok(resp) = client
            .get(sensor_url)
            .header("X-Auth-Token", token)
            .send()
            .await
        {
            if let Ok(json) = from_str::<Value>(&resp.text().await.unwrap_or_default()) {
                if let Some(sensors) = json.get("Sensors").and_then(|s| s.as_array()) {
                    let mut reading = SensorReading {
                        psu_pin: None,
                        cpu_power: None,
                        cpu0_power: None,
                        cpu1_power: None,
                        cpu0_temp: None,
                        cpu1_temp: None,
                        fan_power: None,
                    };

                    for sensor in sensors {
                        let name = sensor.get("Name").and_then(|v| v.as_str()).unwrap_or("");
                        let value = sensor.get("Reading").and_then(|v| v.as_u64());
                        match name {
                            "PSU1_PIN" => reading.psu_pin = value,
                            "CPU_Power" => reading.cpu_power = value,
                            "CPU0_Power" => reading.cpu0_power = value,
                            "CPU1_Power" => reading.cpu1_power = value,
                            "CPU0_Temp" => reading.cpu0_temp = value,
                            "CPU1_Temp" => reading.cpu1_temp = value,
                            "Fan_Power" => reading.fan_power = value,
                            _ => {}
                        }
                    }

                    map.insert(ip.clone(), reading);
                }
            }
        }
    }

    let mut guard = readings.write().await;
    *guard = map;
}

async fn start_ui(
    ips: &[String],
    readings: Arc<RwLock<HashMap<String, SensorReading>>>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let tick_rate = Duration::from_millis(1000);
    let mut last_tick = Instant::now();

    loop {
        let data = readings.read().await;
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(vec![Constraint::Min(3); ips.len()])
                .split(f.area());

            for (i, ip) in ips.iter().enumerate() {
                let reading = data.get(ip);
                let text = match reading {
                    Some(r) => format!(
                        " PSU_PIN: {} W | CPU Tot: {} W  \t\t CPU 0: {} W | CPU 1: {} W \n Fan: {} W \t\t CPU 0 Temp: {} °C | CPU 1 Temp: {} °C",
                        r.psu_pin.unwrap_or(0),
                        r.cpu_power.unwrap_or(0),
                        r.cpu0_power.unwrap_or(0),
                        r.cpu1_power.unwrap_or(0),
                        r.fan_power.unwrap_or(0),
                        r.cpu0_temp.unwrap_or(0),
                        r.cpu1_temp.unwrap_or(0),
                    ),
                    None => " No data available.".to_string(),
                };

                let block = Paragraph::new(text)
                    .block(Block::default().title(ip.to_owned()).borders(Borders::ALL));
                f.render_widget(block, chunks[i]);
            }
        })?;

        if event::poll(tick_rate - last_tick.elapsed())? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
