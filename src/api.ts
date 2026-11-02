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
}
export interface Category { id: number; name: string; position: number; }
export interface CallEntry { id: number; at: string; title: string; kind: string; source: string; }
export interface BreakEntry { id: number; time: string; soundFile: string; label: string; }
export interface Settings { volume: number; ttsGain: number; ttsRate: number; chime: boolean; }
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
export interface ImportReport { people: number; sounds: number; breaks: number; missingSounds: string[]; }
export type SettingKey = "volume" | "tts_gain" | "tts_rate" | "chime";

export interface Backend {
  snapshot(): Promise<Snapshot>;
  addCategory(name: string): Promise<Category>;
  setPersonCategory(id: number, category: string): Promise<void>;
  callPerson(id: number): Promise<void>;
  speak(text: string): Promise<void>;
  stopAll(): Promise<void>;
  setSetting(key: SettingKey, value: string): Promise<void>;
  /** Opens a file picker and imports an old-app backup ZIP. null if the user cancelled. */
  importOldBackup(): Promise<ImportReport | null>;
  onPlayer(cb: (s: PlayerState) => void): () => void;
  onDataChanged(cb: () => void): () => void;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const sub = <T,>(event: string, cb: (p: T) => void) => {
    let off: (() => void) | null = null;
    let dead = false;
    listen<T>(event, (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  };
  return {
    snapshot: () => invoke("snapshot"),
    addCategory: (name) => invoke("add_category", { name }),
    setPersonCategory: (id, category) => invoke("set_person_category", { id, category }),
    callPerson: (id) => invoke("call_person", { id }),
    speak: (text) => invoke("speak", { text }),
    stopAll: () => invoke("stop_all"),
    setSetting: (key, value) => invoke("set_setting", { key, value }),
    async importOldBackup() {
      const path = await open({ title: "Eski uygulamanın yedeğini seçin", filters: [{ name: "Yedek (ZIP)", extensions: ["zip"] }] });
      if (!path) return null;
      return invoke<ImportReport>("import_old_backup", { path });
    },
    onPlayer: (cb) => sub("player", cb),
    onDataChanged: (cb) => sub("data-changed", () => cb()),
  };
}

function mockBackend(): Backend {
  const names = ["Artemis", "Asem", "Ayşe", "Aysel", "Dilek", "Elena", "Erdinç", "Eren", "Fatih", "Fatma", "İbrahim", "Leyla", "Mihriban", "Münevver", "Nina", "Şirin", "Vika", "Burak", "Kaan"];
  const people: Person[] = names.map((name, i) => ({
    id: i + 1, name, category: i > 16 ? "Depo" : "Personeller", color: name === "Nina" ? "#2E7D32" : null, soundFile: "x.mp3", count: (i * 7) % 11, today: 0,
  }));
  people.push({ id: 99, name: "ALUCH", category: "Firmalar", color: null, soundFile: "aluch.mp3", count: 2, today: 0 });
  const categories: Category[] = ["Personeller", "Firmalar", "Depo"].map((name, i) => ({ id: i + 1, name, position: i + 1 }));
  const history: CallEntry[] = [];
  const settings: Settings = { volume: 0.8, ttsGain: 5.5, ttsRate: 0.8, chime: true };
  const playerCbs = new Set<(s: PlayerState) => void>();
  const dataCbs = new Set<() => void>();
  let queue: NowPlaying[] = [];
  let timer = 0;
  let nextId = 1;
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
  const now = () => new Date().toLocaleString("sv-SE").replace("T", " ");
  return {
    snapshot: async () => ({ categories: structuredClone(categories), people: structuredClone(people), history: structuredClone(history), breaks: [{ id: 1, time: "19:12", soundFile: "mola.mp3", label: "Mola" }], settings: { ...settings } }),
    async addCategory(name) {
      name = name.trim();
      if (!name) throw "Kategori adı boş olamaz";
      let c = categories.find((x) => tr.lower(x.name) === tr.lower(name));
      if (!c) { c = { id: categories.length + 1, name, position: categories.length + 1 }; categories.push(c); }
      dataCbs.forEach((cb) => cb());
      return c;
    },
    async setPersonCategory(id, category) {
      people.find((p) => p.id === id)!.category = category;
      dataCbs.forEach((cb) => cb());
    },
    async callPerson(id) {
      const p = people.find((x) => x.id === id)!;
      p.count++; p.today++;
      history.unshift({ id: nextId, at: now(), title: p.name, kind: "person", source: "Bu bilgisayar" });
      queue.push({ id: nextId++, title: p.name, kind: "person", source: "Bu bilgisayar", rep: 1, reps: 2 });
      if (queue.length === 1) play();
      dataCbs.forEach((cb) => cb());
    },
    async speak() { throw "Canlı anons motoru bir sonraki sürümde bağlanacak"; },
    async stopAll() { queue = []; window.clearInterval(timer); emit(idle); },
    async setSetting(key, value) {
      if (key === "volume") settings.volume = +value;
      if (key === "tts_gain") settings.ttsGain = +value;
      if (key === "tts_rate") settings.ttsRate = +value;
      if (key === "chime") settings.chime = value === "true";
    },
    async importOldBackup() { return { people: 37, sounds: 36, breaks: 1, missingSounds: ["ALUCH"] }; },
    onPlayer: (cb) => { playerCbs.add(cb); return () => playerCbs.delete(cb); },
    onDataChanged: (cb) => { dataCbs.add(cb); return () => dataCbs.delete(cb); },
  };
}

export const backendReady: Promise<Backend> = isTauri ? tauriBackend() : Promise.resolve(mockBackend());

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

/** Turkish-aware text helpers (İ/ı, Ş, Ç...). */
export const tr = {
  upper: (s: string) => s.toLocaleUpperCase("tr"),
  lower: (s: string) => s.toLocaleLowerCase("tr"),
  sort: (a: string, b: string) => a.localeCompare(b, "tr"),
};
