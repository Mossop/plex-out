use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::result;

use plex_api::library::MediaItem;
use plex_api::{
    library::{Collection, MetadataItem, Playlist, Season, Show},
    media_container::server::library::{Metadata, MetadataType},
    Server,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Date, OffsetDateTime};
use tokio::fs;
use typeshare::typeshare;
use uuid::Uuid;

trait ListItem<T> {
    fn id(&self) -> T;
}

macro_rules! derive_list_item {
    ($typ:ident) => {
        impl ListItem<u32> for $typ {
            fn id(&self) -> u32 {
                self.id
            }
        }
    };
}

#[derive(Deserialize, Default, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ThumbnailState {
    #[default]
    None,
    #[serde(rename_all = "camelCase")]
    Downloaded { path: PathBuf },
}

impl ThumbnailState {
    pub fn is_none(&self) -> bool {
        matches!(self, ThumbnailState::None)
    }

    pub async fn verify(&mut self, root: &Path) {
        if let ThumbnailState::Downloaded { path } = self {
            let file = root.join(&path);

            match fs::metadata(&file).await {
                Ok(stats) => {
                    if !stats.is_file() {
                        log::error!("'{}' was expected to be a file", path.display());
                    }
                }
                Err(e) => {
                    if e.kind() == ErrorKind::NotFound {
                        *self = ThumbnailState::None;
                    } else {
                        log::error!("Error accessing thumbnail '{}': {e}", path.display());
                    }
                }
            }
        }
    }

    pub async fn delete(&mut self, root: &Path) {
        if let ThumbnailState::Downloaded { path } = self {
            let file = root.join(&path);
            log::trace!("Removing old thumbnail file '{}'", path.display());

            if let Err(e) = fs::remove_file(&file).await {
                log::warn!("Failed to remove file {}: {e}", file.display());
            }

            *self = ThumbnailState::None;
        }
    }
}

fn from_list<'de, D, K, V>(deserializer: D) -> result::Result<HashMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Hash + Eq,
    V: ListItem<K> + Deserialize<'de>,
{
    Ok(Vec::<V>::deserialize(deserializer)?
        .into_iter()
        .map(|v| (v.id(), v))
        .collect())
}

fn into_list<S, K, V>(map: &HashMap<K, V>, serializer: S) -> result::Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    let list: Vec<&V> = map.values().collect();
    list.serialize(serializer)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct CollectionState {
    pub id: u32,
    pub library: u32,
    pub title: String,
    #[typeshare(serialized_as = "Vec<u32>")]
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub items: HashSet<u32>,
    #[serde(with = "time::serde::timestamp")]
    #[typeshare(serialized_as = "number")]
    pub last_updated: OffsetDateTime,
    #[serde(default, skip_serializing_if = "ThumbnailState::is_none")]
    pub thumbnail: ThumbnailState,
}

derive_list_item!(CollectionState);

impl CollectionState {
    pub fn from<T>(collection: &Collection<T>) -> Self {
        Self {
            id: collection.rating_key(),
            library: collection.metadata().library_section_id.unwrap(),
            title: collection.title().to_owned(),
            items: Default::default(),
            last_updated: collection.metadata().updated_at.unwrap(),
            thumbnail: Default::default(),
        }
    }

    pub async fn update<T>(&mut self, collection: &Collection<T>, root: &Path) {
        self.title = collection.title().to_owned();

        self.thumbnail.verify(root).await;

        if self.thumbnail != ThumbnailState::None
            && collection.metadata().updated_at.unwrap() > self.last_updated
        {
            self.thumbnail.delete(root).await;
        }
    }

    pub async fn delete(&mut self, root: &Path) {
        self.thumbnail.verify(root).await;

        if self.thumbnail != ThumbnailState::None {
            self.thumbnail.delete(root).await;
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct PlaylistState {
    pub id: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub videos: Vec<u32>,
}

derive_list_item!(PlaylistState);

impl PlaylistState {
    pub fn from<T>(playlist: &Playlist<T>) -> Self {
        Self {
            id: playlist.rating_key(),
            title: playlist.title().to_owned(),
            videos: Default::default(),
        }
    }

    pub fn update<T>(&mut self, playlist: &Playlist<T>) {
        self.title = playlist.title().to_owned();
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LibraryType {
    Movie,
    Show,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct LibraryState {
    pub id: u32,
    pub title: String,
    #[serde(rename = "type")]
    pub library_type: LibraryType,
}

derive_list_item!(LibraryState);

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct SeasonState {
    pub id: u32,
    pub show: u32,
    pub index: u32,
    pub title: String,
}

derive_list_item!(SeasonState);

impl SeasonState {
    pub fn from(season: &Season) -> Self {
        let metadata = season.metadata();

        Self {
            id: season.rating_key(),
            show: metadata.parent.parent_rating_key.unwrap(),
            index: metadata.index.unwrap(),
            title: season.title().to_owned(),
        }
    }

    pub fn update(&mut self, season: &Season) {
        let metadata = season.metadata();

        self.index = metadata.index.unwrap();
        self.show = metadata.parent.parent_rating_key.unwrap();
        self.title = season.title().to_owned();
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct ShowState {
    pub id: u32,
    pub library: u32,
    pub title: String,
    pub year: u32,
    #[serde(with = "time::serde::timestamp")]
    #[typeshare(serialized_as = "number")]
    pub last_updated: OffsetDateTime,
    #[serde(default, skip_serializing_if = "ThumbnailState::is_none")]
    pub thumbnail: ThumbnailState,
}

derive_list_item!(ShowState);

impl ShowState {
    pub fn from(show: &Show) -> Self {
        let metadata = show.metadata();

        let year = metadata.year.unwrap();
        let title = show.title().to_owned();

        Self {
            id: show.rating_key(),
            library: metadata.library_section_id.unwrap(),
            title,
            year,
            last_updated: metadata.updated_at.unwrap(),
            thumbnail: Default::default(),
        }
    }

    pub async fn update(&mut self, show: &Show, root: &Path) {
        let metadata = show.metadata();

        self.year = metadata.year.unwrap();
        self.title = show.title().to_owned();

        self.thumbnail.verify(root).await;

        if self.thumbnail != ThumbnailState::None
            && metadata.updated_at.unwrap() > self.last_updated
        {
            self.thumbnail.delete(root).await;
        }
    }

    pub async fn delete(&mut self, root: &Path) {
        self.thumbnail.verify(root).await;

        if self.thumbnail != ThumbnailState::None {
            self.thumbnail.delete(root).await;
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct MovieState {
    pub library: u32,
    pub year: u32,
}

impl MovieState {
    pub fn from(metadata: &Metadata) -> Self {
        MovieState {
            library: metadata.library_section_id.unwrap(),
            year: metadata.year.unwrap(),
        }
    }

    pub fn update(&mut self, metadata: &Metadata) {
        self.year = metadata.year.unwrap();
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct EpisodeState {
    pub season: u32,
    pub index: u32,
}

impl EpisodeState {
    pub fn from(metadata: &Metadata) -> Self {
        EpisodeState {
            season: metadata.parent.parent_rating_key.unwrap(),
            index: metadata.index.unwrap(),
        }
    }

    pub fn update(&mut self, metadata: &Metadata) {
        self.season = metadata.parent.parent_rating_key.unwrap();
        self.index = metadata.index.unwrap();
    }
}

#[derive(Deserialize, Default, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DownloadState {
    #[default]
    None,
    #[serde(rename_all = "camelCase")]
    Downloading { path: PathBuf },
    #[serde(rename_all = "camelCase")]
    Transcoding { session_id: String, path: PathBuf },
    #[serde(rename_all = "camelCase")]
    Downloaded { path: PathBuf },
    #[serde(rename_all = "camelCase")]
    Transcoded { path: PathBuf },
}

impl DownloadState {
    fn is_none(&self) -> bool {
        matches!(self, DownloadState::None)
    }

    pub async fn verify(&mut self, server: &Server, root: &Path) {
        let (path, session_id) = match self {
            DownloadState::None => return,
            DownloadState::Downloading { path } => (path, None),
            DownloadState::Transcoding { session_id, path } => (path, Some(session_id)),
            DownloadState::Downloaded { path } => (path, None),
            DownloadState::Transcoded { path } => (path, None),
        };

        let file = root.join(&path);

        match fs::metadata(&file).await {
            Ok(stats) => {
                if !stats.is_file() {
                    log::error!("'{}' was expected to be a file", path.display());
                }

                return;
            }
            Err(e) => {
                if e.kind() != ErrorKind::NotFound {
                    log::error!("Error accessing file '{}': {e}", path.display());
                    return;
                }
            }
        }

        if let Some(session_id) = session_id {
            if let Ok(session) = server.transcode_session(session_id).await {
                if let Err(e) = session.cancel().await {
                    log::warn!("Failed to cancel stale transcode session: {e}");
                }
            }
        }

        *self = DownloadState::None;
    }

    pub async fn delete(&mut self, server: &Server, root: &Path) {
        let (path, session_id) = match self {
            DownloadState::None => return,
            DownloadState::Downloading { path } => (path, None),
            DownloadState::Transcoding { session_id, path } => (path, Some(session_id)),
            DownloadState::Downloaded { path } => (path, None),
            DownloadState::Transcoded { path } => (path, None),
        };

        let file = root.join(&path);

        log::trace!("Removing old video file '{}'", path.display());

        if let Err(e) = fs::remove_file(&file).await {
            log::warn!("Failed to remove file {}: {e}", file.display());
        }

        if let Some(session_id) = session_id {
            if let Ok(session) = server.transcode_session(session_id).await {
                if let Err(e) = session.cancel().await {
                    log::warn!("Failed to cancel stale transcode session: {e}");
                }
            }
        }

        *self = DownloadState::None;
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct VideoPart {
    #[typeshare(serialized_as = "number")]
    pub duration: u64,
    #[serde(default, skip_serializing_if = "DownloadState::is_none")]
    pub download: DownloadState,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum VideoDetail {
    Movie(MovieState),
    Episode(EpisodeState),
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct VideoState {
    pub id: u32,
    pub title: String,
    pub detail: VideoDetail,
    #[typeshare(serialized_as = "string")]
    pub air_date: Date,
    #[serde(default, skip_serializing_if = "ThumbnailState::is_none")]
    pub thumbnail: ThumbnailState,
    #[typeshare(serialized_as = "number")]
    pub media_id: u64,
    #[serde(with = "time::serde::timestamp")]
    #[typeshare(serialized_as = "number")]
    pub last_updated: OffsetDateTime,
    pub parts: Vec<VideoPart>,
}

derive_list_item!(VideoState);

impl VideoState {
    pub fn movie_state(&self) -> &MovieState {
        match self.detail {
            VideoDetail::Movie(ref m) => m,
            VideoDetail::Episode(_) => panic!("Unexpected type"),
        }
    }

    pub fn episode_state(&self) -> &EpisodeState {
        match self.detail {
            VideoDetail::Movie(_) => panic!("Unexpected type"),
            VideoDetail::Episode(ref e) => e,
        }
    }

    pub fn from<M: MediaItem>(item: &M) -> Self {
        let metadata = item.metadata();
        let detail = match metadata.metadata_type {
            Some(MetadataType::Movie) => VideoDetail::Movie(MovieState::from(metadata)),
            Some(MetadataType::Episode) => VideoDetail::Episode(EpisodeState::from(metadata)),
            _ => panic!("Unexpected video type: {:?}", metadata.metadata_type),
        };

        let media = &item.media()[0];
        let parts: Vec<VideoPart> = media
            .parts()
            .iter()
            .map(|p| VideoPart {
                duration: p.metadata().duration.unwrap(),
                download: Default::default(),
            })
            .collect();

        Self {
            id: item.rating_key(),
            title: item.title().to_owned(),
            detail,
            air_date: metadata.originally_available_at.unwrap(),
            thumbnail: Default::default(),
            media_id: media.metadata().id,
            last_updated: metadata.updated_at.unwrap(),
            parts,
        }
    }

    pub async fn update<M: MetadataItem>(&mut self, item: &M, server: &Server, root: &Path) {
        let metadata = item.metadata();
        self.title = item.title().to_owned();

        match self.detail {
            VideoDetail::Movie(ref mut m) => m.update(metadata),
            VideoDetail::Episode(ref mut e) => e.update(metadata),
        }

        self.thumbnail.verify(root).await;
        for part in self.parts.iter_mut() {
            part.download.verify(server, root).await;
        }

        if metadata.updated_at.unwrap() > self.last_updated {
            if self.thumbnail != ThumbnailState::None {
                self.thumbnail.delete(root).await;
            }
            for part in self.parts.iter_mut() {
                if part.download != DownloadState::None {
                    part.download.verify(server, root).await;
                }
            }
        }
    }

    pub async fn delete(&mut self, server: &Server, root: &Path) {
        self.thumbnail.verify(root).await;
        if self.thumbnail != ThumbnailState::None {
            self.thumbnail.delete(root).await;
        }

        for part in self.parts.iter_mut() {
            part.download.verify(server, root).await;
            if part.download != DownloadState::None {
                part.download.delete(server, root).await;
            }
        }
    }
}

#[derive(Deserialize, Default, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<PlaylistState>")]
    pub playlists: HashMap<u32, PlaylistState>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<CollectionState>")]
    pub collections: HashMap<u32, CollectionState>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<LibraryState>")]
    pub libraries: HashMap<u32, LibraryState>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<ShowState>")]
    pub shows: HashMap<u32, ShowState>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<SeasonState>")]
    pub seasons: HashMap<u32, SeasonState>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "into_list",
        deserialize_with = "from_list"
    )]
    #[typeshare(serialized_as = "Vec<VideoState>")]
    pub videos: HashMap<u32, VideoState>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[typeshare]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub servers: HashMap<String, ServerState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            client_id: Uuid::new_v4().braced().to_string(),
            servers: Default::default(),
        }
    }
}
