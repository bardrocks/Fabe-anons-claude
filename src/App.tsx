import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { backendReady, ENGINE_LABEL, errorText, tr, vivid, type Backend, type Category, type NetMode, type Person, type PlayerState, type Release, type Ranked, type Snapshot, type StatsPeriod, type TtsStatus } from "./api";
import { BreaksScreen, HistoryScreen, PeopleScreen, SettingsScreen } from "./screens";

type Page = "anons" | "mola" | "gecmis" | "kisiler" | "ayarlar";
type Toast = (text: string, err?: boolean) => void;

const IDLE: PlayerState = { current: null, queue: [], positionMs: 0, durationMs: 0, level: 0, error: null };
const PAD_COLORS = 6;

/** "2026-11-02 10:42:11" -> "10:42" */
const hhmm = (at: string) => at.slice(11, 16);

function padColor(p: Person) {
  if (p.color) return vivid(p.color);
  const sum = [...p.name].reduce((a, ch) => a + ch.charCodeAt(0), 0);
  return `var(--pad-${(sum % PAD_COLORS) + 1})`;
}

function useToast() {
  const [msg, setMsg] = useState<{ text: string; err: boolean } | null>(null);
  const timer = useRef(0);
  const show = useCallback<Toast>((text, err = false) => {
    setMsg({ text, err });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setMsg(null), err ? 4000 : 2200);
  }, []);
  return [msg, show] as const;
}

function useClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), 5000);
    return () => window.clearInterval(t);
  }, []);
  return now;
}

export default function App() {
  const [api, setApi] = useState<Backend | null>(null);
  const [data, setData] = useState<Snapshot | null>(null);
  const [player, setPlayer] = useState<PlayerState>(IDLE);
  const [page, setPage] = useState<Page>("anons");
  const [release, setRelease] = useState<Release | null>(null);
  const [toast, showToast] = useToast();
  const now = useClock();

  const refresh = useCallback(async (b: Backend) => {
    try { setData(await b.snapshot()); } catch (e) { showToast(errorText(e), true); }
  }, [showToast]);

  useEffect(() => {
    let offs: (() => void)[] = [];
    let dead = false;
    backendReady.then((b) => {
      if (dead) return;
      setApi(b);
      refresh(b);
      offs = [b.onPlayer(setPlayer), b.onDataChanged(() => refresh(b))];
      // Quietly look for a new version a few seconds after start; no network is fine.
      window.setTimeout(() => b.checkUpdate().then((r) => !dead && setRelease(r), () => {}), 4000);
    });
    return () => { dead = true; offs.forEach((f) => f()); };
  }, [refresh]);

  useEffect(() => { if (player.error) showToast(player.error, true); }, [player.error, showToast]);

  const busy = !!player.current || player.queue.length > 0;
  const stop = useCallback(() => {
    if (!api) return;
    api.stopAll();
    if (busy) showToast("Tüm anonslar durduruldu, müzik geri açıldı");
  }, [api, busy, showToast]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") stop(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [stop]);

  const nextBreak = useMemo(() => {
    if (!data?.breaks.length) return null;
    const mins = now.getHours() * 60 + now.getMinutes();
    const toMin = (t: string) => +t.slice(0, 2) * 60 + +t.slice(3, 5);
    const sorted = [...data.breaks].sort((a, b) => toMin(a.time) - toMin(b.time));
    const b = sorted.find((x) => toMin(x.time) > mins) ?? sorted[0];
    const left = (toMin(b.time) - mins + 1440) % 1440;
    return { time: b.time, left: `${Math.floor(left / 60)} sa ${left % 60} dk` };
  }, [data?.breaks, now]);

  const clock = now.toLocaleTimeString("tr-TR", { hour: "2-digit", minute: "2-digit" });

  return (
    <div className={`rack ${page === "anons" ? "" : "page"}`}>
      <Nav page={page} setPage={setPage} mode={data?.settings.netMode ?? "yerel"} release={release} />
      {!api || !data ? (
        <section className="screen plate"><p>Yükleniyor…</p></section>
      ) : page === "anons" ? (
        <>
          <Master player={player} clock={clock} nextBreak={nextBreak} onStop={stop} mode={data.settings.netMode} />
          <Mic api={api} data={data} toast={showToast} onSaved={() => refresh(api)} />
          <Deck api={api} categories={data.categories} people={data.people} player={player} toast={showToast} />
          <Side api={api} data={data} />
        </>
      ) : page === "mola" ? (
        <BreaksScreen api={api} breaks={data.breaks} mode={data.settings.netMode} toast={showToast} />
      ) : page === "gecmis" ? (
        <HistoryScreen api={api} history={data.history} />
      ) : page === "kisiler" ? (
        <PeopleScreen api={api} categories={data.categories} people={data.people} toast={showToast} />
      ) : (
        <SettingsScreen api={api} settings={data.settings} release={release} onRelease={setRelease} toast={showToast} onChanged={() => refresh(api)} />
      )}
      <div className={`toast ${toast ? "show" : ""} ${toast?.err ? "err" : ""}`} role="status">{toast?.text}</div>
    </div>
  );
}

const NAV: { id: Page; label: string; icon: string }[] = [
  { id: "anons", label: "Anons", icon: "M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" },
  { id: "mola", label: "Mola", icon: "M12 9v4l3 2M9 2h6M20 13a8 8 0 1 1-16 0 8 8 0 0 1 16 0" },
  { id: "gecmis", label: "Geçmiş", icon: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5M12 7v5l3 3" },
  { id: "kisiler", label: "Kişiler", icon: "M13 8a4 4 0 1 1-8 0 4 4 0 0 1 8 0M2 21c0-4 3-6 7-6s7 2 7 6M17 11l2 2 4-4" },
  { id: "ayarlar", label: "Ayarlar", icon: "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" },
];

const MODE_LABEL: Record<NetMode, string> = { yerel: "YEREL", sunucu: "SUNUCU", istemci: "İSTEMCİ" };

function Nav({ page, setPage, mode, release }: { page: Page; setPage: (p: Page) => void; mode: NetMode; release: Release | null }) {
  return (
    <nav className="nav plate" aria-label="Bölümler">
      <div className="logo" aria-hidden="true">FABE</div>
      <div className="keys">
        {NAV.map((n) => (
          <button key={n.id} className={`navkey ${page === n.id ? "on" : ""}`} aria-current={page === n.id ? "page" : undefined}
            onClick={() => setPage(n.id)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d={n.icon} /></svg>
            {n.label}
          </button>
        ))}
      </div>
      {release && (
        <button className="navkey update" onClick={() => setPage("ayarlar")} title={`Yeni sürüm ${release.version} var`}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M12 3v12M7 10l5 5 5-5M5 21h14" /></svg>
          {release.version}
        </button>
      )}
      <div className="net"><i />{MODE_LABEL[mode]}</div>
    </nav>
  );
}

function Master({ player, clock, nextBreak, onStop, mode }: {
  player: PlayerState; clock: string; nextBreak: { time: string; left: string } | null; onStop: () => void; mode: NetMode;
}) {
  const cur = player.current;
  const total = cur ? player.durationMs * cur.reps : 0;
  const done = cur ? player.durationMs * (cur.rep - 1) + player.positionMs : 0;
  const progress = total ? Math.min(100, (done / total) * 100) : 0;
  const sub = cur
    ? `${cur.source} · ${cur.reps > 1 ? `${cur.rep}/${cur.reps}. okuma · ` : ""}müzik %10'a kısıldı${player.queue.length ? ` · sırada ${player.queue.length}` : ""}`
    : "Bir pede basın veya mikrofona yazın.";
  return (
    <section className="master plate" aria-live="polite">
      <span className="screw a" /><span className="screw b" /><span className="screw c" /><span className="screw d" />
      <div className="sign">
        <div className="sign-top">
          <span className={`lamp ${cur ? "on" : ""}`}>{cur ? "YAYINDA" : "BEKLEMEDE"}</span>
          <span>
            {nextBreak && <>MOLA {nextBreak.time} · {nextBreak.left} &nbsp;·&nbsp; </>}
            <span className="ok">●</span> {MODE_LABEL[mode]} &nbsp;·&nbsp; {clock}
          </span>
        </div>
        <div className="now">{cur ? tr.upper(cur.kind === "tts" ? "Canlı anons" : cur.title) : "HAZIR"}</div>
        <div className="now-sub">{sub}</div>
        <div className="bar"><i style={{ width: `${progress}%` }} /></div>
      </div>
      <VuMeter level={cur ? player.level : 0} />
      <div className="stopwrap">
        <button className="stop" onClick={onStop}>DURDUR<kbd>Esc</kbd></button>
        <small>TÜMÜNÜ KES</small>
      </div>
    </section>
  );
}

function VuMeter({ level }: { level: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const target = useRef(0);
  target.current = level;
  useEffect(() => {
    const cv = ref.current!;
    const cx = cv.getContext("2d")!;
    const style = getComputedStyle(document.documentElement);
    const [low, mid, high] = ["--ok", "--led", "--rec"].map((v) => style.getPropertyValue(v).trim());
    const shown = [0, 0];
    let raf = 0;
    const draw = (t: number) => {
      const W = cv.width, H = cv.height, seg = 22, gap = 3, sh = (H - gap * (seg + 1)) / seg, cw = (W - gap * 3) / 2;
      cx.clearRect(0, 0, W, H);
      for (let ch = 0; ch < 2; ch++) {
        // Slight per-channel wobble so the two bars don't move as one.
        const wobble = target.current ? 0.9 + 0.1 * Math.sin(t / (90 + ch * 37)) : 0;
        const goal = Math.min(1, target.current * wobble);
        shown[ch] += (goal - shown[ch]) * (goal > shown[ch] ? 0.5 : 0.12);
        const lit = Math.round(shown[ch] * seg);
        for (let i = 0; i < seg; i++) {
          const frac = i / seg;
          cx.globalAlpha = i < lit ? 1 : 0.12;
          cx.fillStyle = frac > 0.82 ? high : frac > 0.6 ? mid : low;
          cx.fillRect(gap + ch * (cw + gap), H - (i + 1) * (sh + gap), cw, sh);
        }
      }
      cx.globalAlpha = 1;
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);
  return <div className="vu"><canvas ref={ref} width={128} height={220} aria-hidden="true" /><small>SEVİYE</small></div>;
}

/** Which engine will read the next announcement, and whether it is ready. */
function EngineBadge({ status, choice }: { status: TtsStatus | null; choice: Snapshot["settings"]["ttsEngine"] }) {
  if (!status) return <div className="engine"><span className="e1"><span className="dot off" />Motor</span><span className="e2">Kontrol ediliyor</span></div>;
  const emaReady = status.ema === "hazir";
  const piperReady = status.piper === "hazir";
  let name: string, dot: string, line: string;
  if (choice === "piper" || (!emaReady && status.ema !== "yukleniyor")) {
    name = "Piper"; dot = piperReady ? "" : "off";
    line = choice === "piper" ? "Ayarlardan seçildi" : piperReady ? "Yedek motor devrede" : "Anons motoru bulunamadı";
  } else if (status.ema === "yukleniyor") {
    name = "ema-lightning"; dot = "wait";
    line = piperReady ? "Yükleniyor · bu arada Piper okur" : "Yükleniyor";
  } else {
    name = "ema-lightning"; dot = "";
    line = piperReady ? "Hazır · yedek: Piper" : "Hazır";
  }
  const last = status.lastEngine ? `Son anons: ${ENGINE_LABEL[status.lastEngine]}` : "";
  const title = [status.emaError && `ema-lightning: ${status.emaError}`, status.lastNote, last].filter(Boolean).join("\n");
  return (
    <div className="engine" title={title || undefined}>
      <span className="e1"><span className={`dot ${dot}`} />{name}</span>
      <span className="e2">{line}</span>
    </div>
  );
}

function Mic({ api, data, toast, onSaved }: { api: Backend; data: Snapshot; toast: Toast; onSaved: () => void }) {
  const [text, setText] = useState("");
  const [gain, setGain] = useState(data.settings.ttsGain);
  const [rate, setRate] = useState(data.settings.ttsRate);
  const [vol, setVol] = useState(Math.round(data.settings.volume * 100));
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<TtsStatus | null>(null);

  useEffect(() => {
    let alive = true;
    const load = () => api.ttsStatus().then((s) => alive && setStatus(s), () => {});
    load();
    const t = window.setInterval(load, 4000);
    return () => { alive = false; window.clearInterval(t); };
  }, [api]);

  const speak = async () => {
    const t = text.trim();
    if (!t) return toast("Önce okunacak metni yazın");
    if (busy) return;
    setBusy(true);
    try {
      const r = await api.speak(t);
      setText("");
      if (r.note) toast(r.note, r.engine === "piper" && !r.note.startsWith("Sunucuya"));
      api.ttsStatus().then(setStatus, () => {});
    } catch (e) {
      toast(errorText(e), true);
    } finally {
      setBusy(false);
    }
  };
  const save = (key: "tts_gain" | "tts_rate", v: number) => api.setSetting(key, String(v)).then(onSaved, (e) => toast(errorText(e), true));

  return (
    <section className="mic plate" aria-label="Canlı anons">
      <div className="mic-ico" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" strokeWidth="1.8" strokeLinecap="round"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3" /></svg>
      </div>
      <label className="mic-field">
        <span>Canlı anons</span>
        <input value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && speak()}
          placeholder="Okunacak metni yazın, Enter'a basın" autoComplete="off" />
      </label>
      <button className="speak" onClick={speak} disabled={busy}>{busy ? "HAZIRLANIYOR" : "SESLENDİR"}</button>
      <div className="mic-opts">
        <label className="ctl">Güç
          <input type="range" min={1} max={15} step={0.5} value={gain} onChange={(e) => setGain(+e.target.value)}
            onPointerUp={() => save("tts_gain", gain)} onKeyUp={() => save("tts_gain", gain)} />
          <b>{gain.toFixed(1).replace(".0", "")}x</b>
        </label>
        <label className="ctl">Hız
          <input type="range" min={0.5} max={1.5} step={0.05} value={rate} onChange={(e) => setRate(+e.target.value)}
            onPointerUp={() => save("tts_rate", rate)} onKeyUp={() => save("tts_rate", rate)} />
          <b>{rate.toFixed(2)}x</b>
        </label>
        <label className="ctl">Ana ses
          <input type="range" min={0} max={100} value={vol}
            onChange={(e) => { setVol(+e.target.value); api.setSetting("volume", String(+e.target.value / 100)); }} />
          <b>{vol}</b>
        </label>
        <EngineBadge status={status} choice={data.settings.ttsEngine} />
      </div>
    </section>
  );
}

/** "all" or a category name. */
type Tab = string;

function Deck({ api, categories, people, player, toast }: {
  api: Backend; categories: Category[]; people: Person[]; player: PlayerState; toast: Toast;
}) {
  const [tab, setTab] = useState<Tab>("all");
  const [q, setQ] = useState("");
  const [adding, setAdding] = useState<string | null>(null);
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "/" && document.activeElement?.tagName !== "INPUT") { e.preventDefault(); search.current?.focus(); }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  // A category that only exists on a person (e.g. typed in an old backup) still gets a tab.
  const cats = useMemo(() => {
    const names = categories.map((c) => c.name);
    for (const p of people) if (!names.some((n) => tr.lower(n) === tr.lower(p.category))) names.push(p.category);
    return names;
  }, [categories, people]);
  const inCat = (p: Person, c: string) => tr.lower(p.category) === tr.lower(c);

  const sorted = useMemo(() => [...people].sort((a, b) => tr.sort(a.name, b.name)), [people]);
  const needle = tr.lower(q.trim());
  const list = sorted.filter((p) => (tab === "all" || inCat(p, tab)) && (!needle || tr.lower(p.name).includes(needle)));

  const isPlaying = (p: Person) => player.current?.kind === "person" && player.current.title === p.name;
  const call = async (p: Person) => {
    if (isPlaying(p)) return toast(`${p.name} zaten yayında`);
    try {
      const note = await api.callPerson(p.id);
      if (note) toast(note);
      else if (player.current) toast(`${p.name} sıraya alındı`);
    } catch (e) { toast(errorText(e), true); }
  };

  const addCategory = async () => {
    const name = (adding ?? "").trim();
    setAdding(null);
    if (!name) return;
    try {
      const c = await api.addCategory(name);
      setTab(c.name);
      toast(`"${c.name}" kategorisi eklendi. Kişileri Kişiler ekranından bu kategoriye taşıyabilirsiniz.`);
    } catch (e) { toast(errorText(e), true); }
  };

  const pad = (p: Person) => {
    const playing = isPlaying(p);
    const queued = !playing && player.queue.some((x) => x.kind === "person" && x.title === p.name);
    const label = playing ? "● Yayında" : queued ? "Sırada" : !p.soundFile ? "Ses yok" : "";
    return (
      <button key={p.id} className={`card ${playing ? "playing" : queued ? "queued" : ""}`} style={{ "--c": padColor(p) } as CSSProperties}
        disabled={!p.soundFile} onClick={() => call(p)} title={`${p.name} · toplam ${p.count}, bugün ${p.today}`}>
        <span className="lbl"><span className="name">{tr.upper(p.name)}</span><span className="state">{label}</span></span>
        <span className="count">{String(p.count).padStart(2, "0")}</span>
      </button>
    );
  };

  const tabs: [Tab, string, number][] = [
    ["all", "Tümü", people.length],
    ...cats.map((c): [Tab, string, number] => [c, c, people.filter((p) => inCat(p, c)).length]),
  ];

  return (
    <main className="deck plate">
      <div className="toolbar">
        <label className="search">
          <input ref={search} value={q} onChange={(e) => setQ(e.target.value)} placeholder="İsim ara, Enter ile çağır" autoComplete="off"
            onKeyDown={(e) => { if (e.key === "Enter" && list[0]) { call(list[0]); setQ(""); } }} />
          <kbd>/</kbd>
        </label>
        <div className="tabs" role="tablist">
          {tabs.map(([k, l, n]) => (
            <button key={k} className="tab" role="tab" aria-selected={tab === k} onClick={() => setTab(k)}>{l}<span>{n}</span></button>
          ))}
          {adding === null ? (
            <button className="tab add" title="Yeni kategori ekle" onClick={() => setAdding("")}>+</button>
          ) : (
            <input className="tab new" autoFocus value={adding} placeholder="Kategori adı" maxLength={24}
              onChange={(e) => setAdding(e.target.value)} onBlur={addCategory}
              onKeyDown={(e) => { if (e.key === "Enter") addCategory(); if (e.key === "Escape") { e.stopPropagation(); setAdding(null); } }} />
          )}
        </div>
      </div>
      <div className="pads">
        {!people.length ? (
          <div className="empty">Henüz kimse yok. Ayarlar ekranından eski uygulamanın yedeğini yükleyebilirsiniz.</div>
        ) : !list.length ? (
          <div className="empty">{q ? `“${q}” ile eşleşen kimse yok.` : "Bu kategoride henüz kimse yok."}</div>
        ) : tab === "all" ? (
          cats.map((c) => {
            const group = list.filter((p) => inCat(p, c));
            return group.length > 0 && <Fragment key={c}><div className="section-label">{c}</div>{group.map(pad)}</Fragment>;
          })
        ) : list.map(pad)}
      </div>
    </main>
  );
}

const PERIODS: [StatsPeriod, string][] = [["bugun", "Bugün"], ["hafta", "Hafta"], ["ay", "Ay"], ["tumu", "Tümü"]];

function Side({ api, data }: { api: Backend; data: Snapshot }) {
  const [period, setPeriod] = useState<StatsPeriod>("tumu");
  const [ranked, setRanked] = useState<Ranked[] | null>(null);
  useEffect(() => {
    if (period === "tumu") return setRanked(null);
    let alive = true;
    api.stats(period).then((r) => alive && setRanked(r), () => alive && setRanked([]));
    return () => { alive = false; };
  }, [api, period, data.history]);
  const top: { key: string | number; name: string; count: number }[] = ranked
    ? ranked.map((r) => ({ key: r.personId ?? r.name, name: r.name, count: r.count }))
    : [...data.people].sort((a, b) => b.count - a.count || tr.sort(a.name, b.name)).slice(0, 5).map((p) => ({ key: p.id, name: p.name, count: p.count }));
  const max = top[0]?.count || 1;
  const total = ranked ? ranked.reduce((a, r) => a + r.count, 0) : data.people.reduce((a, p) => a + p.count, 0);
  const today = new Date().toLocaleDateString("sv-SE");
  const recent = data.history.filter((h) => h.at.startsWith(today)).slice(0, 12);
  return (
    <aside className="side">
      <section className="panel plate">
        <h2>En çok çağrılanlar <span>{ranked ? "ilk 5" : "toplam"} {total}</span></h2>
        <div className="periods" role="tablist" aria-label="Dönem">
          {PERIODS.map(([k, l]) => (
            <button key={k} role="tab" aria-selected={period === k} className={period === k ? "on" : ""} onClick={() => setPeriod(k)}>{l}</button>
          ))}
        </div>
        <ol className="stats">
          {!top.length && <li className="none">Bu dönemde anons yok</li>}
          {top.map((p) => (
            <li key={p.key}>
              <span className="sn">{tr.upper(p.name)}</span>
              <span className="track"><span className="fill" style={{ width: `${(p.count / max) * 100}%` }} /></span>
              <span className="sv">{p.count}</span>
            </li>
          ))}
        </ol>
      </section>
      <section className="panel plate grow">
        <h2>Son anonslar <span>bugün</span></h2>
        <ul className="list">
          {recent.length ? recent.map((h) => (
            <li key={h.id}>
              <span className="t">{hhmm(h.at)}</span>
              <span className="what">{h.kind === "tts" ? `Canlı: ${h.title}` : h.title}</span>
              <span className="src">{h.source}</span>
            </li>
          )) : <li className="none">Bugün henüz anons yok</li>}
        </ul>
      </section>
    </aside>
  );
}
