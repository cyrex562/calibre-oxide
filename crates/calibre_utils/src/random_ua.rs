use crate::resources::get_path;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use rand::distr::weighted::WeightedIndex;
use rand::prelude::*;
use std::sync::Mutex;
use lazy_static::lazy_static;

#[derive(Deserialize, Clone)]
struct UserAgentData {
    common_user_agents: Vec<String>,
    user_agents_popularity: HashMap<String, f64>,
    desktop_platforms: Vec<String>,
}

lazy_static! {
    static ref UA_DATA: Mutex<Option<UserAgentData>> = Mutex::new(None);
    static ref ENGLISH_WORDS: Mutex<Option<Vec<String>>> = Mutex::new(None);
}

fn get_ua_data() -> Option<UserAgentData> {
    let mut data = UA_DATA.lock().unwrap();
    if data.is_none() {
        if let Some(path) = get_path("user-agent-data.json", false) {
             if let Ok(content) = fs::read_to_string(path) {
                 if let Ok(parsed) = serde_json::from_str(&content) {
                     *data = Some(parsed);
                 }
             }
        }
    }
    data.clone()
}

fn get_english_words() -> Option<Vec<String>> {
    let mut words = ENGLISH_WORDS.lock().unwrap();
    if words.is_none() {
        if let Some(path) = get_path("common-english-words.txt", true) {
             if let Ok(content) = fs::read_to_string(path) {
                 let list: Vec<String> = content.lines().map(|s| s.trim().to_string()).collect();
                 *words = Some(list);
             }
        }
    }
    words.clone()
}

pub fn common_user_agents() -> Vec<String> {
    get_ua_data().map(|d| d.common_user_agents).unwrap_or_default()
}

pub fn random_common_chrome_user_agent() -> String {
    if let Some(data) = get_ua_data() {
        let chrome_uas: Vec<&String> = data.common_user_agents.iter()
            .filter(|ua| ua.contains("Chrome/"))
            .collect();
            
        if chrome_uas.is_empty() { return String::new(); }
        
        let popularities = &data.user_agents_popularity;
        let weights: Vec<f64> = chrome_uas.iter()
            .map(|ua| *popularities.get(*ua).unwrap_or(&1.0))
            .collect();
            
        if let Ok(dist) = WeightedIndex::new(&weights) {
            let mut rng = rand::rng();
            let idx = dist.sample(&mut rng); // usize
            return chrome_uas[idx].clone();
        }
        // Fallback uniform
        let mut rng = rand::rng();
        return chrome_uas.choose(&mut rng).unwrap().to_string();
    }
    String::new()
}

pub fn random_english_text(max_sentences: usize, min_words: usize, max_words: usize) -> String {
    if let Some(words) = get_english_words() {
        if words.is_empty() { return String::new(); }
        let mut rng = rand::rng();
        let num_sent = rng.random_range(1..=max_sentences);
        
        let mut sentences = Vec::new();
        for _ in 0..num_sent {
             let num_words = rng.random_range(min_words..=max_words);
             let sent_words: Vec<&String> = (0..num_words).map(|_| words.choose(&mut rng).unwrap()).collect();
             let mut sent = sent_words.into_iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
             // Capitalize first letter?
             if let Some(first) = sent.chars().next() {
                 let cap = first.to_uppercase().to_string();
                 sent.replace_range(0..1, &cap);
             }
             sent.push('.');
             sentences.push(sent);
        }
        return sentences.join(" ");
    }
    String::new()
}

pub fn random_desktop_platform() -> String {
    get_ua_data().map(|d| {
        let mut rng = rand::rng();
        d.desktop_platforms.choose(&mut rng).cloned().unwrap_or_default()
    }).unwrap_or_default()
}

pub fn common_english_word_ua() -> String {
    if let Some(words) = get_english_words() {
        if words.is_empty() { return String::new(); }
        let mut rng = rand::rng();
        let w1 = words.choose(&mut rng).unwrap();
        let mut w2 = w1;
        while w2 == w1 {
            w2 = words.choose(&mut rng).unwrap();
        }
        return format!("{}/{}", w1, w2);
    }
    String::new()
}
