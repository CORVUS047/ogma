//! Looking up cover art on the internet.
//!
//! Three services, tried in order of how precisely they can answer:
//!
//! 1. **Cover Art Archive**, when the file carries a MusicBrainz release id — an exact lookup, no
//!    guessing involved.
//! 2. **MusicBrainz** search to turn an artist and album into release ids, then the Cover Art
//!    Archive again.
//! 3. **iTunes Search**, which often has art for releases MusicBrainz does not cover.
//!
//! None of them need an account or a key. What leaves this machine is the artist and album names
//! from the file's own tags, or a MusicBrainz id — nothing else, and only when the user has turned
//! this on.
//!
//! MusicBrainz asks callers for an identifying user agent and no more than one request a second;
//! both are honoured below.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Identifies this player to the services it asks. MusicBrainz requires something meaningful here.
const USER_AGENT: &str = concat!("ogma/", env!("CARGO_PKG_VERSION"), " (terminal music player)");

/// Give up on a request that takes longer than this, so a slow service cannot stall the fill.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// MusicBrainz asks for at most one request per second; a little over is politer still.
const MUSICBRAINZ_INTERVAL: Duration = Duration::from_millis(1100);

/// Largest image accepted, matching what the local search allows.
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// How many MusicBrainz candidates to try art for before giving up on an album.
const MAX_RELEASE_CANDIDATES: usize = 3;

/// Least search score a MusicBrainz release must have before it is believed.
///
/// The search is fuzzy: asking for an album that does not exist still returns whatever came closest.
/// A cover from the wrong album is worse than no cover at all, so candidates are checked rather than
/// taken on trust.
const MIN_SEARCH_SCORE: u64 = 80;

/// What is known about the release being looked up, from the file's own tags.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct Query {
    pub artist: Option<String>,
    pub album: Option<String>,
    /// MusicBrainz release id, if the file carries one.
    pub release_mbid: Option<String>,
}

impl Query {
    /// Whether there is enough here to ask anyone anything.
    pub(super) fn is_usable(&self) -> bool {
        self.release_mbid.is_some() || (self.artist.is_some() && self.album.is_some())
    }
}

/// An image, and the service it came from.
#[derive(Clone, Debug)]
pub(super) struct Found {
    pub data: Vec<u8>,
    pub source: String,
}

/// How many requests have been made, for tests and for anyone curious about the traffic.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// Requests issued since the player started.
pub fn requests_made() -> u64 {
    REQUESTS.load(Ordering::Relaxed)
}

/// Look for cover art for `query`, or `None` when nothing turns up.
pub(super) fn cover_for(query: &Query) -> Option<Found> {
    if !query.is_usable() {
        return None;
    }

    // An exact id beats any search.
    if let Some(mbid) = &query.release_mbid
        && let Some(found) = cover_art_archive(mbid)
    {
        return Some(found);
    }

    if let (Some(artist), Some(album)) = (&query.artist, &query.album) {
        for mbid in musicbrainz_releases(artist, album) {
            if let Some(found) = cover_art_archive(&mbid) {
                return Some(found);
            }
        }

        if let Some(found) = itunes(artist, album) {
            return Some(found);
        }
    }

    None
}

/// The shared HTTP agent. One connection pool, one user agent, one timeout.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into()
    })
}

/// Fetch an image, checking that it really is one and that it is not absurdly large.
fn fetch_image(url: &str) -> Option<Vec<u8>> {
    REQUESTS.fetch_add(1, Ordering::Relaxed);

    let mut response = agent().get(url).call().ok()?;

    // Only images, whatever the URL promised.
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !content_type.starts_with("image/") {
        return None;
    }

    // The limit is enforced while reading, so an enormous body is never held in full.
    let data = response.body_mut().with_config().limit(MAX_IMAGE_BYTES).read_to_vec().ok()?;

    (!data.is_empty()).then_some(data)
}

/// Fetch JSON, for the two search services.
fn fetch_json(url: &str) -> Option<Value> {
    REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Read the bytes and parse them here rather than through ureq's optional json support, which
    // keeps the dependency's feature set at its default.
    let body = agent()
        .get(url)
        .header("accept", "application/json")
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()
        .ok()?;

    serde_json::from_slice(&body).ok()
}

/// The front cover the Cover Art Archive holds for a MusicBrainz release.
fn cover_art_archive(mbid: &str) -> Option<Found> {
    if !is_mbid(mbid) {
        return None;
    }

    // `front-500` is a 500 pixel rendition: large enough to look right, small enough to embed in
    // every track of an album without bloating the files.
    let url = format!("https://coverartarchive.org/release/{mbid}/front-500");

    Some(Found { data: fetch_image(&url)?, source: format!("coverartarchive.org ({mbid})") })
}

/// Release ids MusicBrainz offers for an artist and album.
fn musicbrainz_releases(artist: &str, album: &str) -> Vec<String> {
    wait_for_musicbrainz();

    let query = format!(
        "release:\"{}\" AND artist:\"{}\"",
        escape_lucene(album),
        escape_lucene(artist)
    );
    let url = format!(
        "https://musicbrainz.org/ws/2/release/?fmt=json&limit={MAX_RELEASE_CANDIDATES}&query={}",
        percent_encode(&query)
    );

    let Some(body) = fetch_json(&url) else {
        return Vec::new();
    };

    body.get("releases")
        .and_then(Value::as_array)
        .map(|releases| {
            releases
                .iter()
                .filter(|release| is_the_release_asked_for(release, artist, album))
                .filter_map(|release| release.get("id")?.as_str().map(str::to_owned))
                .filter(|id| is_mbid(id))
                .take(MAX_RELEASE_CANDIDATES)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a search result really is the release that was asked for.
///
/// Both the score MusicBrainz assigns and the names themselves have to agree: the score alone lets
/// through a confident match on something else entirely.
fn is_the_release_asked_for(release: &Value, artist: &str, album: &str) -> bool {
    let score = release.get("score").and_then(Value::as_u64).unwrap_or(0);
    if score < MIN_SEARCH_SCORE {
        return false;
    }

    let Some(title) = release.get("title").and_then(Value::as_str) else {
        return false;
    };

    if !names_agree(title, album) {
        return false;
    }

    // Any of the credited artists will do: "Artist feat. Other" should still match "Artist".
    release
        .get("artist-credit")
        .and_then(Value::as_array)
        .is_some_and(|credits| {
            credits.iter().any(|credit| {
                credit
                    .get("name")
                    .or_else(|| credit.get("artist")?.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|name| names_agree(name, artist))
            })
        })
}

/// Whether two names refer to the same thing, allowing for punctuation, case, and one being a
/// fuller form of the other — "OK Computer" against "OK Computer OKNOTOK 1997 2017", say.
fn names_agree(left: &str, right: &str) -> bool {
    let left = normalize_name(left);
    let right = normalize_name(right);

    if left.is_empty() || right.is_empty() {
        return false;
    }

    left == right || left.starts_with(&right) || right.starts_with(&left)
}

/// Lowercase, alphanumeric only: what is left of a name once spelling differences are set aside.
fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Album art from the iTunes Search API, which needs no key.
fn itunes(artist: &str, album: &str) -> Option<Found> {
    let term = percent_encode(&format!("{artist} {album}"));
    let url =
        format!("https://itunes.apple.com/search?media=music&entity=album&limit=1&term={term}");

    let body = fetch_json(&url)?;
    let result = body.get("results")?.as_array()?.first()?;

    // Like MusicBrainz, iTunes answers a hopeless query with its closest guess, so the names it
    // returns are checked before its art is believed.
    let found_artist = result.get("artistName").and_then(Value::as_str).unwrap_or_default();
    let found_album = result.get("collectionName").and_then(Value::as_str).unwrap_or_default();

    if !names_agree(found_artist, artist) || !names_agree(found_album, album) {
        return None;
    }

    let artwork = result.get("artworkUrl100")?.as_str()?;

    // The API returns a 100 pixel thumbnail; the same path serves larger renditions.
    let large = artwork.replace("100x100bb", "600x600bb");

    Some(Found { data: fetch_image(&large)?, source: "itunes.apple.com".to_string() })
}

/// Hold off until a second has passed since the last MusicBrainz request.
fn wait_for_musicbrainz() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);

    let mut last = LAST.lock().unwrap_or_else(|err| err.into_inner());

    if let Some(previous) = *last {
        let since = previous.elapsed();

        if since < MUSICBRAINZ_INTERVAL {
            std::thread::sleep(MUSICBRAINZ_INTERVAL - since);
        }
    }

    *last = Some(Instant::now());
}

/// Whether a string looks like a MusicBrainz id, so nothing else ends up in a URL path.
fn is_mbid(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Percent-encode a query string value, keeping the unreserved characters.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

/// Escape the characters Lucene treats as syntax, so a title cannot alter the query's meaning.
fn escape_lucene(value: &str) -> String {
    const SPECIAL: &[char] = &[
        '\\', '+', '-', '!', '(', ')', ':', '^', '[', ']', '"', '{', '}', '~', '*', '?', '|', '&',
        '/',
    ];

    let mut out = String::with_capacity(value.len());

    for c in value.chars() {
        if SPECIAL.contains(&c) {
            out.push('\\');
        }

        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_looking_ids_reach_a_url() {
        assert!(is_mbid("76df3287-6cda-33eb-8e9a-044b5e15ffdd"));
        assert!(!is_mbid("../../etc/passwd"));
        assert!(!is_mbid("short"));
        assert!(!is_mbid("76df3287-6cda-33eb-8e9a-044b5e15ffd!"));
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(percent_encode("Sigur Rós"), "Sigur%20R%C3%B3s");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(percent_encode("plain-text_1.0~"), "plain-text_1.0~");
    }

    #[test]
    fn lucene_syntax_in_a_title_is_escaped() {
        assert_eq!(escape_lucene("Now: That's What I Call Music!"), r"Now\: That's What I Call Music\!");
        assert_eq!(escape_lucene("AC/DC"), r"AC\/DC");
        assert_eq!(escape_lucene("plain"), "plain");
    }

    #[test]
    fn a_query_needs_either_an_id_or_both_names() {
        assert!(!Query::default().is_usable());
        assert!(!Query { artist: Some("a".into()), ..Query::default() }.is_usable());
        assert!(Query { artist: Some("a".into()), album: Some("b".into()), ..Query::default() }
            .is_usable());
        assert!(Query { release_mbid: Some("id".into()), ..Query::default() }.is_usable());
    }

    #[test]
    fn names_are_compared_past_punctuation_and_case() {
        assert!(names_agree("OK Computer", "ok computer"));
        assert!(names_agree("Sgt. Pepper's", "Sgt Peppers"));
        // A fuller edition of the same album still counts.
        assert!(names_agree("OK Computer OKNOTOK 1997 2017", "OK Computer"));

        assert!(!names_agree("Kid A", "Amnesiac"));
        assert!(!names_agree("", "something"));
    }

    #[test]
    fn a_confident_match_on_the_wrong_album_is_refused() {
        // What a fuzzy search does with a query nobody can satisfy: a high score on something else.
        let wrong = serde_json::json!({
            "score": 100,
            "title": "Some Other Record",
            "artist-credit": [{ "name": "Some Other Band" }],
        });

        assert!(!is_the_release_asked_for(&wrong, "Zzqx Nonexistent Band", "Qqqq No Such Record"));

        let right = serde_json::json!({
            "score": 100,
            "title": "OK Computer",
            "artist-credit": [{ "name": "Radiohead" }],
        });

        assert!(is_the_release_asked_for(&right, "Radiohead", "OK Computer"));

        // The right names but a poor score is not believed either.
        let unsure = serde_json::json!({
            "score": 40,
            "title": "OK Computer",
            "artist-credit": [{ "name": "Radiohead" }],
        });

        assert!(!is_the_release_asked_for(&unsure, "Radiohead", "OK Computer"));
    }

    #[test]
    fn an_unusable_query_asks_nobody_anything() {
        let before = requests_made();

        assert!(cover_for(&Query::default()).is_none());
        assert_eq!(requests_made(), before, "no request should have been made");
    }
}
