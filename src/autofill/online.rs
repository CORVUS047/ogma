//! Looking up cover art and genres on the internet.
//!
//! Four sources, tried in order of how precisely they can answer:
//!
//! 1. **Cover Art Archive**, when the file carries a MusicBrainz release id — an exact lookup, no
//!    guessing involved.
//! 2. **MusicBrainz** search to turn an artist and album into release ids, then the Cover Art
//!    Archive again.
//! 3. **iTunes Search**, which often has art for releases MusicBrainz does not cover.
//! 4. **A YouTube video's thumbnail**, as a last resort — see [`youtube_thumbnail`]. Not a cover but
//!    a frame from whatever somebody uploaded, so it is reached only when every source that files
//!    album art properly has come back with nothing.
//!
//! Genres come from the same two searches: MusicBrainz publishes the genres its users have voted
//! for on a release and on the release group it belongs to, and iTunes names one genre per album.
//! Artwork and genres are separate switches, so one can be on while the other is off.
//!
//! None of them need an account or a key. What leaves this machine is the artist and album names
//! from the file's own tags, or a MusicBrainz id — nothing else, and only when the user has turned
//! this on. The YouTube search goes out through yt-dlp, which is what this player already asks about
//! YouTube; a machine without it simply finds nothing at that step.
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

/// How many YouTube results are considered when falling back to a thumbnail.
///
/// The first few are where a search puts the album upload if there is one; past that the results are
/// other people's playlists and live sets, whose thumbnails have nothing to do with the record.
const MAX_YOUTUBE_RESULTS: usize = 3;

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

/// What a lookup should go and find.
///
/// Artwork and genres are separate switches, and a file can be missing one without missing the
/// other, so what is asked for is said rather than assumed: a release whose cover is already on
/// disk costs no image fetch when only its genre is wanted.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Wanted {
    pub artwork: bool,
    pub genre: bool,
}

impl Wanted {
    /// Whether there is anything here to go and ask for.
    pub(super) fn any(self) -> bool {
        self.artwork || self.genre
    }

    /// What `self` wants that `other` already holds the answer to.
    fn still_missing(self, answer: &Answer) -> Self {
        Wanted {
            artwork: self.artwork && answer.artwork.is_none(),
            genre: self.genre && answer.genre.is_none(),
        }
    }
}

/// What a lookup found. Either half can be empty, whether or not it was asked for.
#[derive(Clone, Debug, Default)]
pub(super) struct Answer {
    pub artwork: Option<Found>,
    pub genre: Option<String>,
}

impl Answer {
    /// Take whatever `other` found that this does not already have.
    pub(super) fn absorb(&mut self, other: Answer) {
        if self.artwork.is_none() {
            self.artwork = other.artwork;
        }
        if self.genre.is_none() {
            self.genre = other.genre;
        }
    }
}

/// How many requests have been made, for tests and for anyone curious about the traffic.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// Requests issued since the player started.
pub fn requests_made() -> u64 {
    REQUESTS.load(Ordering::Relaxed)
}

/// Ask the services about `query`, for whichever of a cover and a genre `wanted` asks for.
///
/// One pass serves both: the MusicBrainz search that turns an artist and album into release ids is
/// made once and its candidates answer either question, and the iTunes fallback reads art and
/// genre out of a single search result. Asking for the two separately would double the traffic,
/// and MusicBrainz is held to one request a second.
pub(super) fn look_up(query: &Query, wanted: Wanted) -> Answer {
    let mut answer = Answer::default();

    if !wanted.any() || !query.is_usable() {
        return answer;
    }

    // An exact id beats any search.
    if let Some(mbid) = &query.release_mbid {
        if wanted.artwork {
            answer.artwork = cover_art_archive(mbid);
        }
        if wanted.genre {
            answer.genre = musicbrainz_genre(mbid);
        }
    }

    let (Some(artist), Some(album)) = (&query.artist, &query.album) else {
        return answer;
    };

    let mut missing = wanted.still_missing(&answer);

    if missing.any() {
        for mbid in musicbrainz_releases(artist, album) {
            if missing.artwork {
                answer.artwork = cover_art_archive(&mbid);
            }
            if missing.genre {
                answer.genre = musicbrainz_genre(&mbid);
            }

            missing = wanted.still_missing(&answer);

            if !missing.any() {
                return answer;
            }
        }
    }

    // iTunes often has art for releases MusicBrainz does not cover, and files every album under a
    // genre; one search answers both.
    if missing.any()
        && let Some(result) = itunes_album(artist, album)
    {
        if missing.artwork {
            answer.artwork = itunes_artwork(&result);
        }
        if missing.genre {
            answer.genre = itunes_genre(&result);
        }
    }

    // Everything that files album art has been asked by now. A video thumbnail is what is left: a
    // worse picture than a cover, and better than the blank square the player draws without one.
    // Nothing here answers the genre question — YouTube has no such thing to give.
    if wanted.artwork && answer.artwork.is_none() {
        answer.artwork = youtube_thumbnail(artist, album);
    }

    answer
}

/// The thumbnail of a YouTube video of this release, when one can be found.
///
/// The last thing tried, and the least trustworthy: a thumbnail is whatever frame or picture the
/// uploader chose, in the shape a video is rather than square, and for a full-album upload it is
/// usually the cover while for anything else it is usually not. So the result has to name both the
/// artist and the album before its thumbnail is taken — a search for a record nobody has uploaded
/// still comes back with a page of other things, and a frame from the wrong video is worse than
/// leaving the file alone.
///
/// yt-dlp does the searching, being what this player already asks about YouTube. A machine without
/// it finds nothing here, which is the same as a search that matched nothing.
fn youtube_thumbnail(artist: &str, album: &str) -> Option<Found> {
    if !crate::ytdl::available() {
        return None;
    }

    // Counted like any other request: it is traffic leaving the machine, whoever carries it.
    REQUESTS.fetch_add(1, Ordering::Relaxed);

    let results = crate::ytdl::search(&format!("{artist} {album}"), MAX_YOUTUBE_RESULTS).ok()?;

    let track = results
        .iter()
        .find(|track| mentions(&track.title, track.uploader.as_deref(), artist, album))?;

    // The id is about to go into a URL, so only the shape YouTube's ids actually have is accepted.
    if track.id.is_empty()
        || !track.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }

    // Thumbnails sit at fixed names, largest first. The big ones are not made for every video;
    // `hqdefault` is always there.
    for size in ["maxresdefault", "sddefault", "hqdefault"] {
        let url = format!("https://i.ytimg.com/vi/{}/{size}.jpg", track.id);

        if let Some(data) = fetch_image(&url) {
            return Some(Found { data, source: format!("youtube.com ({})", track.id) });
        }
    }

    None
}

/// Whether a YouTube result is about this release.
///
/// Titles are free text — `Artist - Album (Full Album) [HD]` — so the names have to be found inside
/// one rather than matched against it, and the uploader counts as part of the text: a channel named
/// for the artist is how an upload titled only `OK Computer` says whose it is.
fn mentions(title: &str, uploader: Option<&str>, artist: &str, album: &str) -> bool {
    let text = normalize_name(&format!("{title} {}", uploader.unwrap_or_default()));
    let artist = normalize_name(artist);
    let album = normalize_name(album);

    if artist.is_empty() || album.is_empty() {
        return false;
    }

    text.contains(&artist) && text.contains(&album)
}

/// The genre MusicBrainz holds for a release: the most voted for of the release's own, or of its
/// release group's when the release itself has none.
fn musicbrainz_genre(mbid: &str) -> Option<String> {
    if !is_mbid(mbid) {
        return None;
    }

    wait_for_musicbrainz();

    let url =
        format!("https://musicbrainz.org/ws/2/release/{mbid}?fmt=json&inc=genres+release-groups");
    let body = fetch_json(&url)?;

    best_genre(body.get("genres"))
        .or_else(|| best_genre(body.get("release-group")?.get("genres")))
}

/// The most voted for of a list of MusicBrainz genres.
///
/// Ties are broken by name so that two runs over the same album agree; a genre nobody has voted for
/// is still a genre, and counts as zero.
fn best_genre(genres: Option<&Value>) -> Option<String> {
    let mut best: Option<(u64, String)> = None;

    for genre in genres?.as_array()? {
        let Some(name) = genre.get("name").and_then(Value::as_str) else {
            continue;
        };
        let count = genre.get("count").and_then(Value::as_u64).unwrap_or(0);

        let better = match &best {
            None => true,
            Some((best_count, best_name)) => {
                count > *best_count || (count == *best_count && name < best_name.as_str())
            }
        };

        if better {
            best = Some((count, name.to_string()));
        }
    }

    best.map(|(_, name)| title_case(&name))
}

/// MusicBrainz writes its genres in lowercase; a tag reads better with the words capitalised.
fn title_case(genre: &str) -> String {
    genre
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();

            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// The iTunes Search API's entry for an album, which needs no key.
///
/// Like MusicBrainz, iTunes answers a hopeless query with its closest guess, so the names it
/// returns are checked here, once, before anything it says is believed.
fn itunes_album(artist: &str, album: &str) -> Option<Value> {
    let term = percent_encode(&format!("{artist} {album}"));
    let url =
        format!("https://itunes.apple.com/search?media=music&entity=album&limit=1&term={term}");

    let body = fetch_json(&url)?;
    let result = body.get("results")?.as_array()?.first()?;

    let found_artist = result.get("artistName").and_then(Value::as_str).unwrap_or_default();
    let found_album = result.get("collectionName").and_then(Value::as_str).unwrap_or_default();

    if !names_agree(found_artist, artist) || !names_agree(found_album, album) {
        return None;
    }

    Some(result.clone())
}

/// Album art from an iTunes search result.
fn itunes_artwork(result: &Value) -> Option<Found> {
    let artwork = result.get("artworkUrl100")?.as_str()?;

    // The API returns a 100 pixel thumbnail; the same path serves larger renditions.
    let large = artwork.replace("100x100bb", "600x600bb");

    Some(Found { data: fetch_image(&large)?, source: "itunes.apple.com".to_string() })
}

/// The genre iTunes files an album under.
fn itunes_genre(result: &Value) -> Option<String> {
    let genre = result.get("primaryGenreName")?.as_str()?.trim();

    // iTunes files everything it has no genre of its own for under `Music`, which says nothing.
    if genre.is_empty() || genre.eq_ignore_ascii_case("music") {
        return None;
    }

    Some(genre.to_string())
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
    fn a_youtube_result_has_to_name_the_release() {
        assert!(mentions("Radiohead - OK Computer (Full Album)", None, "Radiohead", "OK Computer"));

        // The channel is part of what a result says it is: an upload titled with the album alone is
        // named by whose channel it sits on.
        assert!(mentions("OK Computer [full album]", Some("Radiohead"), "Radiohead", "OK Computer"));

        // A thumbnail from any of these is a frame of something else.
        assert!(!mentions("Radiohead - Kid A", None, "Radiohead", "OK Computer"));
        assert!(!mentions("10 hours of rain sounds", Some("Sleep"), "Radiohead", "OK Computer"));

        // Nothing to go on is not a match.
        assert!(!mentions("Radiohead - OK Computer", None, "", "OK Computer"));
        assert!(!mentions("", None, "Radiohead", "OK Computer"));
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
        let everything = Wanted { artwork: true, genre: true };

        let answer = look_up(&Query::default(), everything);

        assert!(answer.artwork.is_none());
        assert!(answer.genre.is_none());
        assert_eq!(requests_made(), before, "no request should have been made");
    }

    #[test]
    fn a_lookup_that_wants_nothing_asks_nothing() {
        let before = requests_made();
        let query = Query {
            artist: Some("Radiohead".to_string()),
            album: Some("OK Computer".to_string()),
            release_mbid: None,
        };

        assert!(look_up(&query, Wanted::default()).artwork.is_none());
        assert_eq!(requests_made(), before, "nothing wanted, so nobody asked");
    }

    #[test]
    fn the_most_voted_for_genre_wins_and_ties_go_by_name() {
        let genres = serde_json::json!([
            { "name": "shoegaze", "count": 3 },
            { "name": "dream pop", "count": 9 },
            { "name": "noise rock", "count": 9 },
        ]);

        // Capitalised on the way out: MusicBrainz writes its genres in lowercase.
        assert_eq!(best_genre(Some(&genres)), Some("Dream Pop".to_string()));

        let unvoted = serde_json::json!([{ "name": "ambient" }]);
        assert_eq!(best_genre(Some(&unvoted)), Some("Ambient".to_string()));

        assert_eq!(best_genre(Some(&serde_json::json!([]))), None);
        assert_eq!(best_genre(None), None);
    }

    #[test]
    fn itunes_shelf_genres_that_say_nothing_are_refused() {
        let result = serde_json::json!({ "primaryGenreName": "Dance" });
        assert_eq!(itunes_genre(&result), Some("Dance".to_string()));

        // What iTunes files an album under when it has nothing better to say.
        assert_eq!(itunes_genre(&serde_json::json!({ "primaryGenreName": "Music" })), None);
        assert_eq!(itunes_genre(&serde_json::json!({ "primaryGenreName": " " })), None);
        assert_eq!(itunes_genre(&serde_json::json!({})), None);
    }
}
