// Bridge to the Rust core. In a plain browser (npm run dev without Tauri) a small mock
// stands in, so the interface can be designed and tested without the desktop app.

export type Kind = "person" | "tts" | "break";

export interface Person {
  id: number;
  name: string;
  /** Category name: "Personeller", "Firmalar" or one the user added (e.g. "Depo"). */
  category: string;
  color: string | null;
  soundFile: string | null;
  count: number;
  today: number;
  /** Place on the main screen set by drag and drop; null = alphabetical, after the ordered ones. */
  position: number | null;
}
export interface Category { id: number; name: string; position: number; }
export interface CallEntry { id: number; at: string; title: string; kind: string; source: string; }
export interface BreakEntry { id: number; time: string; soundFile: string; label: string; }
export type NetMode = "yerel" | "sunucu" | "istemci";
export type TtsEngineChoice = "otomatik" | "ema" | "piper";
export type TtsEngine = "ema" | "piper";
export interface TtsStatus {
  ema: "yok" | "kapali" | "yukleniyor" | "hazir" | "hata";
  emaError: string | null;
  piper: "yok" | "hazir";
  lastEngine: TtsEngine | null;
  lastNote: string | null;
}
export type StatsPeriod = "tumu" | "bugun" | "hafta" | "ay";
export interface Ranked { personId: number | null; name: string; count: number; }
export interface SpeakResult { engine: TtsEngine; note: string | null; }
export const ENGINE_LABEL: Record<TtsEngine, string> = { ema: "ema-lightning", piper: "Piper" };
export interface Settings {
  volume: number;
  ttsGain: number;
  ttsRate: number;
  ttsEngine: TtsEngineChoice;
  chime: boolean;
  netMode: NetMode;
  netServerIp: string;
  netPort: number;
  pcName: string;
  /** Person button size on the main screen, percent (70-200). */
  padSize: number;
  version: string;
}
export interface Snapshot { categories: Category[]; people: Person[]; history: CallEntry[]; breaks: BreakEntry[]; settings: Settings; }
export interface QueueView { id: number; title: string; kind: Kind; source: string; }
export interface NowPlaying extends QueueView { rep: number; reps: number; }
export interface PlayerState {
  current: NowPlaying | null;
  queue: QueueView[];
  positionMs: number;
  durationMs: number;
  level: number;
  error: string | null;
}
export interface RestoreReport {
  kind: "v4" | "eski";
  people: number;
  sounds: number;
  breaks: number;
  missingSounds: string[];
  safetyBackup: string;
}
export interface BackupFile { path: string; name: string; created: string; size: number; }
export interface ServerStatus { listening: boolean; port: number; error: string | null; clients: { name: string; ip: string; lastSeen: string }[]; }
export interface FoundServer { name: string; ip: string; port: number; version: string; }
export interface Release { version: string; notes: string; url: string; sha256: string; }
export interface UpdateProgress { step: string; done: number; total: number; }
export interface PersonInput { name: string; category: string; color: string | null; soundFile: string | null; }
export interface BreakInput { time: string; label: string; soundFile: string | null; }
export type SettingKey = "volume" | "tts_gain" | "tts_rate" | "chime" | "net_mode" | "net_server_ip" | "net_port" | "pc_name" | "tts_engine" | "pad_size";

export interface Backend {
  snapshot(): Promise<Snapshot>;
  // Categories
  addCategory(name: string): Promise<Category>;
  renameCategory(id: number, name: string): Promise<void>;
  /** Returns a note like "3 kişi Personeller kategorisine taşındı" (empty if nobody moved). */
  deleteCategory(id: number): Promise<string>;
  moveCategory(id: number, step: -1 | 1): Promise<void>;
  // People (announcement buttons)
  setPersonCategory(id: number, category: string): Promise<void>;
  createPerson(p: PersonInput & { soundFile: string }): Promise<number>;
  updatePerson(id: number, p: PersonInput): Promise<void>;
  deletePerson(id: number): Promise<void>;
  /** Saves the button order after a drag and drop (first id comes first). */
  reorderPeople(ids: number[]): Promise<void>;
  // Sounds
  /** Opens a file picker, copies the chosen audio into the app. null if cancelled. */
  pickSound(nameHint: string): Promise<string | null>;
  previewSound(file: string): Promise<void>;
  // Breaks
  createBreak(b: BreakInput & { soundFile: string }): Promise<void>;
  updateBreak(id: number, b: BreakInput): Promise<void>;
  deleteBreak(id: number): Promise<void>;
  // Playback
  /** Returns a note to show (e.g. "Ayşe sunucuya gönderildi"), or "" when played here. */
  callPerson(id: number): Promise<string>;
  /** Reads the text twice with ema-lightning, or Piper if that fails. */
  speak(text: string): Promise<SpeakResult>;
  ttsStatus(): Promise<TtsStatus>;
  /** Most-called names in the last day / 7 days / 30 days. */
  stats(period: Exclude<StatsPeriod, "tumu">): Promise<Ranked[]>;
  /** Latest 200 announcements, or those matching the search. */
  searchHistory(query: string): Promise<CallEntry[]>;
  stopAll(): Promise<void>;
  setSetting(key: SettingKey, value: string): Promise<void>;
  // Backups
  /** Asks where to save, then writes a full backup. Returns the path, or null if cancelled. */
  createBackup(): Promise<string | null>;
  /** Asks for a ZIP (this app's or the old app's) and restores it. null if cancelled. */
  restoreBackup(path?: string): Promise<RestoreReport | null>;
  listBackups(): Promise<BackupFile[]>;
  showInFolder(path: string): Promise<void>;
  // Network
  testConnection(ip: string, port: number): Promise<string>;
  localIps(): Promise<string[]>;
  /** FABE servers found on the local network (no IP or port typing needed). */
  findServers(): Promise<FoundServer[]>;
  useServer(ip: string, port: number): Promise<void>;
  netStatus(): Promise<ServerStatus | null>;
  // Updates
  checkUpdate(): Promise<Release | null>;
  installUpdate(r: Release): Promise<void>;
  // Events
  onPlayer(cb: (s: PlayerState) => void): () => void;
  onDataChanged(cb: () => void): () => void;
  onUpdateProgress(cb: (p: UpdateProgress) => void): () => void;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const AUDIO = ["mp3", "wav", "ogg", "oga", "opus", "flac", "m4a", "aac", "mp4", "webm", "amr", "wma", "3gp", "mpeg", "aiff"];

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { open, save } = await import("@tauri-apps/plugin-dialog");
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  const sub = <T,>(event: string, cb: (p: T) => void) => {
    let off: (() => void) | null = null;
    let dead = false;
    listen<T>(event, (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  };
  const stamp = () => new Date().toLocaleString("sv-SE").slice(0, 16).replace(/[-: ]/g, (c) => (c === " " ? "_" : ""));
  return {
    snapshot: () => invoke("snapshot"),
    addCategory: (name) => invoke("add_category", { name }),
    renameCategory: (id, name) => invoke("rename_category", { id, name }),
    deleteCategory: (id) => invoke("delete_category", { id }),
    moveCategory: (id, step) => invoke("move_category", { id, step }),
    setPersonCategory: (id, category) => invoke("set_person_category", { id, category }),
    createPerson: (p) => invoke("create_person", { ...p }),
    updatePerson: (id, p) => invoke("update_person", { id, ...p }),
    deletePerson: (id) => invoke("delete_person", { id }),
    async pickSound(nameHint) {
      const path = await open({ title: "Ses dosyası seçin", filters: [{ name: "Ses dosyası", extensions: AUDIO }, { name: "Tüm dosyalar", extensions: ["*"] }] });
      if (!path) return null;
      return invoke<string>("import_sound", { path, nameHint });
    },
    previewSound: (file) => invoke("preview_sound", { file }),
    createBreak: (b) => invoke("create_break", { ...b }),
    updateBreak: (id, b) => invoke("update_break", { id, ...b }),
    deleteBreak: (id) => invoke("delete_break", { id }),
    callPerson: (id) => invoke("call_person", { id }),
    reorderPeople: (ids) => invoke("reorder_people", { ids }),
    speak: (text) => invoke("speak", { text }),
    ttsStatus: () => invoke("tts_status"),
    stats: (period) => invoke("stats", { period }),
    searchHistory: (query) => invoke("search_history", { query }),
    stopAll: () => invoke("stop_all"),
    setSetting: (key, value) => invoke("set_setting", { key, value }),
    async createBackup() {
      const path = await save({ title: "Yedeği nereye kaydedelim?", defaultPath: `FABE_Anons_Yedek_${stamp()}.zip`, filters: [{ name: "Yedek (ZIP)", extensions: ["zip"] }] });
      if (!path) return null;
      await invoke("create_backup", { path });
      return path;
    },
    async restoreBackup(given) {
      const path = given ?? (await open({ title: "Geri yüklenecek yedeği seçin", filters: [{ name: "Yedek (ZIP)", extensions: ["zip"] }] }));
      if (!path) return null;
      return invoke<RestoreReport>("restore_backup", { path });
    },
    listBackups: () => invoke("list_backups"),
    showInFolder: (path) => revealItemInDir(path),
    testConnection: (ip, port) => invoke("test_connection", { ip, port }),
    localIps: () => invoke("local_ips"),
    findServers: () => invoke("find_servers"),
    useServer: (ip, port) => invoke("use_server", { ip, port }),
    netStatus: () => invoke("net_status"),
    checkUpdate: () => invoke("check_update"),
    installUpdate: (release) => invoke("install_update", { release }),
    onPlayer: (cb) => sub("player", cb),
    onDataChanged: (cb) => sub("data-changed", () => cb()),
    onUpdateProgress: (cb) => sub("update-progress", cb),
  };
}

function mockBackend(): Backend {
  const names = ["Artemis", "Asem", "Ayşe", "Aysel", "Dilek", "Elena", "Erdinç", "Eren", "Fatih", "Fatma", "İbrahim", "Leyla", "Mihriban", "Münevver", "Nina", "Şirin", "Vika", "Burak", "Kaan"];
  const people: Person[] = names.map((name, i) => ({
    id: i + 1, name, category: i > 16 ? "Depo" : "Personeller", color: name === "Nina" ? "#2E7D32" : null, soundFile: `${tr.lower(name)}.mp3`, count: (i * 7) % 11, today: 0, position: null,
  }));
  people.push({ id: 99, name: "ALUCH", category: "Firmalar", color: null, soundFile: "aluch.mp3", count: 2, today: 0, position: null });
  let categories: Category[] = ["Personeller", "Firmalar", "Depo"].map((name, i) => ({ id: i + 1, name, position: i + 1 }));
  let breaks: BreakEntry[] = [{ id: 1, time: "12:30", soundFile: "ogle-molasi.mp3", label: "Öğle molası" }, { id: 2, time: "19:12", soundFile: "mola1.mp3", label: "Mola" }];
  const history: CallEntry[] = [];
  const settings: Settings = { volume: 0.8, ttsGain: 5.5, ttsRate: 0.8, ttsEngine: "otomatik", chime: true, netMode: "yerel", netServerIp: "", netPort: 55555, pcName: "KASA-1", padSize: 100, version: "4.2.1" };
  const playerCbs = new Set<(s: PlayerState) => void>();
  const dataCbs = new Set<() => void>();
  const changed = () => dataCbs.forEach((cb) => cb());
  let queue: NowPlaying[] = [];
  let timer = 0;
  let nextId = 100;
  const idle: PlayerState = { current: null, queue: [], positionMs: 0, durationMs: 0, level: 0, error: null };
  const emit = (s: PlayerState) => playerCbs.forEach((cb) => cb(s));
  const play = () => {
    const cur = queue[0];
    if (!cur) return emit(idle);
    const dur = 2200;
    const start = performance.now();
    window.clearInterval(timer);
    timer = window.setInterval(() => {
      const t = performance.now() - start;
      const rep = Math.min(cur.reps, 1 + Math.floor(t / dur));
      emit({ current: { ...cur, rep }, queue: queue.slice(1), positionMs: t % dur, durationMs: dur,
        level: 0.4 + 0.5 * Math.abs(Math.sin(t / 170)), error: null });
      if (t >= dur * cur.reps) { window.clearInterval(timer); queue.shift(); play(); }
    }, 50);
  };
  const enqueue = (title: string, kind: Kind, source: string, reps: number) => {
    queue.push({ id: nextId++, title, kind, source, rep: 1, reps });
    if (queue.length === 1) play();
  };
  const now = () => new Date().toLocaleString("sv-SE").replace("T", " ");
  const sameCat = (a: string, b: string) => tr.lower(a) === tr.lower(b);
  return {
    snapshot: async () => ({ categories: structuredClone(categories), people: structuredClone(people), history: structuredClone(history), breaks: structuredClone(breaks), settings: { ...settings } }),
    async addCategory(name) {
      name = name.trim();
      if (!name) throw "Kategori adı boş olamaz";
      let c = categories.find((x) => sameCat(x.name, name));
      if (!c) { c = { id: nextId++, name, position: categories.length + 1 }; categories.push(c); }
      changed();
      return c;
    },
    async renameCategory(id, name) {
      const c = categories.find((x) => x.id === id)!;
      if (categories.some((x) => x.id !== id && sameCat(x.name, name))) throw `"${name}" adında bir kategori zaten var`;
      people.forEach((p) => { if (sameCat(p.category, c.name)) p.category = name; });
      c.name = name; changed();
    },
    async deleteCategory(id) {
      if (categories.length < 2) throw "Son kategori silinemez";
      const c = categories.find((x) => x.id === id)!;
      categories = categories.filter((x) => x.id !== id);
      const target = categories[0].name;
      let n = 0;
      people.forEach((p) => { if (sameCat(p.category, c.name)) { p.category = target; n++; } });
      changed();
      return n ? `${n} kişi "${target}" kategorisine taşındı` : "";
    },
    async moveCategory(id, step) {
      const i = categories.findIndex((x) => x.id === id), j = i + step;
      if (j < 0 || j >= categories.length) return;
      [categories[i], categories[j]] = [categories[j], categories[i]];
      categories.forEach((c, k) => (c.position = k + 1));
      changed();
    },
    async setPersonCategory(id, category) { people.find((p) => p.id === id)!.category = category; changed(); },
    async createPerson(p) {
      if (people.some((x) => x.name === p.name.trim())) throw `"${p.name}" adında biri zaten var`;
      const id = nextId++;
      people.push({ id, name: p.name.trim(), category: p.category, color: p.color, soundFile: p.soundFile, count: 0, today: 0, position: null });
      changed();
      return id;
    },
    async updatePerson(id, p) {
      const x = people.find((y) => y.id === id)!;
      Object.assign(x, { name: p.name.trim(), category: p.category, color: p.color, soundFile: p.soundFile ?? x.soundFile });
      changed();
    },
    async reorderPeople(ids) { ids.forEach((id, i) => { const p = people.find((x) => x.id === id); if (p) p.position = i; }); changed(); },
    async deletePerson(id) { people.splice(people.findIndex((p) => p.id === id), 1); changed(); },
    async pickSound(hint) { return `${tr.lower(hint || "ses")}-${Date.now()}.mp3`; },
    async previewSound() { enqueue("Deneme", "break", "Dinleme", 1); },
    async createBreak(b) { breaks.push({ id: nextId++, time: b.time, label: b.label, soundFile: b.soundFile }); breaks.sort((a, c) => a.time.localeCompare(c.time)); changed(); },
    async updateBreak(id, b) { const x = breaks.find((y) => y.id === id)!; Object.assign(x, { time: b.time, label: b.label, soundFile: b.soundFile ?? x.soundFile }); changed(); },
    async deleteBreak(id) { breaks = breaks.filter((b) => b.id !== id); changed(); },
    async callPerson(id) {
      const p = people.find((x) => x.id === id)!;
      p.count++; p.today++;
      history.unshift({ id: nextId, at: now(), title: p.name, kind: "person", source: "Bu bilgisayar" });
      enqueue(p.name, "person", "Bu bilgisayar", 2);
      changed();
      return "";
    },
    async speak(text) {
      history.unshift({ id: nextId++, at: now(), title: text, kind: "tts", source: "Bu bilgisayar" });
      enqueue(text, "tts", "Bu bilgisayar", 2);
      changed();
      return { engine: "ema", note: null };
    },
    async stats(period) {
      const n = period === "bugun" ? 1 : period === "hafta" ? 3 : 6;
      return people.slice(0, 5).map((p, i) => ({ personId: p.id, name: p.name, count: Math.max(1, n * (5 - i)) }));
    },
    async searchHistory(query) {
      const q = tr.lower(query.trim());
      return history.filter((h) => !q || tr.lower(h.title).includes(q) || tr.lower(h.source).includes(q)).slice(0, 200);
    },
    async ttsStatus() { return { ema: "hazir", emaError: null, piper: "hazir", lastEngine: "ema", lastNote: null }; },
    async stopAll() { queue = []; window.clearInterval(timer); emit(idle); },
    async setSetting(key, value) {
      if (key === "volume") settings.volume = +value;
      if (key === "tts_gain") settings.ttsGain = +value;
      if (key === "tts_rate") settings.ttsRate = +value;
      if (key === "chime") settings.chime = value === "true";
      if (key === "net_mode") settings.netMode = value as NetMode;
      if (key === "net_server_ip") settings.netServerIp = value;
      if (key === "net_port") settings.netPort = +value;
      if (key === "pc_name") settings.pcName = value;
      if (key === "pad_size") settings.padSize = +value;
      if (key === "tts_engine") settings.ttsEngine = value as TtsEngineChoice;
    },
    async createBackup() { return "C:\\Users\\kasa\\Desktop\\FABE_Anons_Yedek.zip"; },
    async restoreBackup() { return { kind: "eski", people: 37, sounds: 36, breaks: 1, missingSounds: ["ALUCH"], safetyBackup: "...\\yedekler\\FABE_Anons_Yedek_4.0.0_geri-yukleme-oncesi.zip" }; },
    async listBackups() { return [{ path: "x", name: "FABE_Anons_Yedek_4.0.0_20261102_131500_guncelleme-oncesi.zip", created: "2026-11-02 13:15", size: 5_400_000 }]; },
    async showInFolder() {},
    async testConnection(ip) { if (!ip) throw "Önce sunucunun IP adresini yazın"; return "Bağlantı başarılı (4 ms)"; },
    async localIps() { return ["192.168.1.20"]; },
    async findServers() { await new Promise((r) => setTimeout(r, 600)); return [{ name: "KASA-ANA", ip: "192.168.1.20", port: 55555, version: "4.2.1" }]; },
    async useServer(ip, port) { settings.netServerIp = ip; settings.netPort = port; },
    async netStatus() { return settings.netMode === "sunucu" ? { listening: true, port: settings.netPort, error: null, clients: [{ name: "KASA-1", ip: "192.168.1.31", lastSeen: "14:05" }] } : null; },
    async checkUpdate() { return { version: "4.3.0", notes: "Canlı anons motoru eklendi.", url: "", sha256: "" }; },
    async installUpdate() { throw "Tarayıcı önizlemesinde güncelleme yapılamaz"; },
    onPlayer: (cb) => { playerCbs.add(cb); return () => playerCbs.delete(cb); },
    onDataChanged: (cb) => { dataCbs.add(cb); return () => dataCbs.delete(cb); },
    onUpdateProgress: () => () => {},
  };
}


export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

/** Turkish-aware text helpers (İ/ı, Ş, Ç...). */
export const tr = {
  upper: (s: string) => s.toLocaleUpperCase("tr"),
  lower: (s: string) => s.toLocaleLowerCase("tr"),
  sort: (a: string, b: string) => a.localeCompare(b, "tr"),
};

/** Colors offered for buttons (old app's palette plus the Konsol accents). */
export const BUTTON_COLORS = [
  "#ffb02e", "#ff6fa8", "#ff3b30", "#8f7bff", "#5aa9ff", "#4fd1c5",
  "#7ed957", "#22c55e", "#00e0ff", "#ffe14d", "#ff7a1a", "#d946ef",
];

/**
 * Makes a button color bright enough to stand out on the dark pads. Colors carried over from
 * the old app are often dark (navy, dark green) and nearly disappear on a dark background.
 */
export function vivid(color: string): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return color;
  const n = parseInt(m[1], 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let h = 0;
  if (d) h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  h = (h * 60 + 360) % 360;
  const l0 = (max + min) / 2;
  const s0 = d ? d / (1 - Math.abs(2 * l0 - 1)) : 0;
  // Greys stay grey (just lighter); real colors get full saturation and mid lightness.
  const s = s0 < 0.12 ? s0 : Math.max(s0, 0.85);
  const l = Math.min(Math.max(l0, 0.55), 0.68);
  return `hsl(${Math.round(h)} ${Math.round(s * 100)}% ${Math.round(l * 100)}%)`;
}

// Last, so the helpers above exist before the mock uses them.
export const backendReady: Promise<Backend> = isTauri ? tauriBackend() : Promise.resolve(mockBackend());
