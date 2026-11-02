// Management screens: Mola, Geçmiş, Kişiler (+ kategoriler), Ayarlar (yedek, ağ, güncelleme).
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BUTTON_COLORS, errorText, tr,
  type Backend, type BackupFile, type BreakEntry, type CallEntry, type Category, type NetMode,
  type Person, type Release, type RestoreReport, type ServerStatus, type Settings, type TtsEngineChoice,
  type TtsStatus, type UpdateProgress, ENGINE_LABEL, vivid,
} from "./api";

type Toast = (m: string, err?: boolean) => void;

/** A button that asks "Emin misiniz?" on the first click and acts on the second. */
function ConfirmButton({ label, confirm = "Emin misiniz?", onConfirm, className = "btn small danger" }: {
  label: string; confirm?: string; onConfirm: () => void; className?: string;
}) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = window.setTimeout(() => setArmed(false), 3000);
    return () => window.clearTimeout(t);
  }, [armed]);
  return (
    <button className={`${className} ${armed ? "armed" : ""}`} onClick={() => (armed ? (setArmed(false), onConfirm()) : setArmed(true))}>
      {armed ? confirm : label}
    </button>
  );
}

/** Pick a sound file, show its name, listen to it. */
function SoundPicker({ api, file, hint, onChange, toast }: {
  api: Backend; file: string | null; hint: string; onChange: (f: string) => void; toast: Toast;
}) {
  const [busy, setBusy] = useState(false);
  const pick = async () => {
    setBusy(true);
    try {
      const f = await api.pickSound(hint);
      if (f) onChange(f);
    } catch (e) { toast(errorText(e), true); } finally { setBusy(false); }
  };
  return (
    <div className="row">
      <button className="btn" disabled={busy} onClick={pick}>{busy ? "Okunuyor…" : file ? "Başka ses seç" : "Ses dosyası seç"}</button>
      {file && <button className="btn" onClick={() => api.previewSound(file).catch((e) => toast(errorText(e), true))}>▶ Dinle</button>}
      <span className="filename">{file ?? "Henüz ses seçilmedi"}</span>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return <div className="field"><span className="field-label">{label}</span>{children}</div>;
}

/* ---------------- Mola ---------------- */

interface BreakDraft { id: number | null; time: string; label: string; soundFile: string | null; }

export function BreaksScreen({ api, breaks, mode, toast }: { api: Backend; breaks: BreakEntry[]; mode: NetMode; toast: Toast }) {
  const [draft, setDraft] = useState<BreakDraft | null>(null);
  const sorted = [...breaks].sort((a, b) => a.time.localeCompare(b.time));

  const save = async () => {
    if (!draft) return;
    if (!draft.time) return toast("Saat seçin", true);
    try {
      if (draft.id === null) {
        if (!draft.soundFile) return toast("Bu mola için bir ses dosyası seçin", true);
        await api.createBreak({ time: draft.time, label: draft.label, soundFile: draft.soundFile });
        toast(`${draft.time} molası eklendi`);
      } else {
        await api.updateBreak(draft.id, { time: draft.time, label: draft.label, soundFile: draft.soundFile });
        toast("Mola güncellendi");
      }
      setDraft(null);
    } catch (e) { toast(errorText(e), true); }
  };

  return (
    <section className="screen plate">
      <div className="screen-head">
        <h1>MOLA</h1>
        <button className="btn primary" onClick={() => setDraft({ id: null, time: "", label: "Mola", soundFile: null })}>+ Mola saati ekle</button>
      </div>
      <p>Uygulama açıkken, buradaki her saatte seçilen ses dosyası kendiliğinden çalınır (gongsuz; müzik o sırada kısılır).</p>
      {mode === "istemci" && (
        <p className="warn">Bu bilgisayar istemci modunda, bu yüzden molalar burada çalmaz. Mola saatlerini hoparlöre bağlı sunucu bilgisayarda tanımlayın.</p>
      )}

      {draft && (
        <div className="group editor">
          <h3>{draft.id === null ? "Yeni mola" : "Molayı düzenle"}</h3>
          <div className="row">
            <Field label="Saat"><input className="input" type="time" value={draft.time} onChange={(e) => setDraft({ ...draft, time: e.target.value })} /></Field>
            <Field label="Açıklama"><input className="input" value={draft.label} placeholder="Öğle molası" onChange={(e) => setDraft({ ...draft, label: e.target.value })} /></Field>
          </div>
          <SoundPicker api={api} file={draft.soundFile} hint={`mola ${draft.time}`} toast={toast} onChange={(f) => setDraft({ ...draft, soundFile: f })} />
          <div className="row">
            <button className="btn primary" onClick={save}>Kaydet</button>
            <button className="btn" onClick={() => setDraft(null)}>Vazgeç</button>
          </div>
        </div>
      )}

      <table className="table">
        <thead><tr><th>Saat</th><th>Açıklama</th><th>Ses</th><th /></tr></thead>
        <tbody>
          {sorted.length ? sorted.map((b) => (
            <tr key={b.id}>
              <td className="mono big">{b.time}</td>
              <td>{b.label}</td>
              <td className="filename">{b.soundFile}</td>
              <td className="actions">
                <button className="btn small" onClick={() => api.previewSound(b.soundFile).catch((e) => toast(errorText(e), true))}>▶ Dinle</button>
                <button className="btn small" onClick={() => setDraft({ id: b.id, time: b.time, label: b.label, soundFile: b.soundFile })}>Düzenle</button>
                <ConfirmButton label="Sil" onConfirm={() => api.deleteBreak(b.id).then(() => toast(`${b.time} molası silindi`), (e) => toast(errorText(e), true))} />
              </td>
            </tr>
          )) : <tr><td colSpan={4} className="dim">Kayıtlı mola yok. "+ Mola saati ekle" ile ekleyin.</td></tr>}
        </tbody>
      </table>
    </section>
  );
}

/* ---------------- Geçmiş ---------------- */

const KIND_LABEL: Record<string, string> = { person: "İsim", tts: "Canlı", break: "Mola" };

export function HistoryScreen({ api, history }: { api: Backend; history: CallEntry[] }) {
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<CallEntry[] | null>(null);
  useEffect(() => {
    let alive = true;
    // Short pause so typing doesn't search on every key.
    const t = window.setTimeout(() => api.searchHistory(query).then((r) => alive && setRows(r), () => {}), query ? 200 : 0);
    return () => { alive = false; window.clearTimeout(t); };
  }, [api, query, history]);
  const list = rows ?? history;
  return (
    <section className="screen plate">
      <h1>GEÇMİŞ</h1>
      <div className="row">
        <input className="input grow" type="search" value={query} onChange={(e) => setQuery(e.target.value)}
          placeholder="İsim, metin, bilgisayar adı veya tarih (2026-11-02) ara" aria-label="Geçmişte ara" />
      </div>
      <p>{query ? `${list.length} sonuç${list.length === 200 ? " (ilk 200)" : ""}` : "Son 200 anons. Hangi bilgisayardan gönderildiği de görünür."}</p>
      <table className="table">
        <thead><tr><th>Zaman</th><th>Tür</th><th>Anons</th><th>Kaynak</th></tr></thead>
        <tbody>
          {list.length ? list.map((h) => (
            <tr key={h.id}><td className="mono">{h.at.slice(0, 16)}</td><td>{KIND_LABEL[h.kind] ?? h.kind}</td><td>{h.title}</td><td>{h.source}</td></tr>
          )) : <tr><td colSpan={4} className="dim">{query ? "Eşleşen anons yok." : "Henüz anons yok."}</td></tr>}
        </tbody>
      </table>
    </section>
  );
}

/* ---------------- Kişiler + Kategoriler ---------------- */

interface PersonDraft { id: number | null; name: string; category: string; color: string | null; soundFile: string | null; }

function ColorPicker({ value, onChange }: { value: string | null; onChange: (c: string | null) => void }) {
  return (
    <div className="swatches">
      <button className={`swatch none ${value === null ? "on" : ""}`} title="Otomatik renk" onClick={() => onChange(null)}>A</button>
      {BUTTON_COLORS.map((c) => (
        <button key={c} className={`swatch ${value?.toLowerCase() === c.toLowerCase() ? "on" : ""}`} style={{ background: vivid(c) }} title={c} onClick={() => onChange(c)} />
      ))}
      <label className="swatch custom" title="Başka bir renk seç">
        <input type="color" value={value ?? "#ffb02e"} onChange={(e) => onChange(e.target.value)} />+
      </label>
    </div>
  );
}

function CategoryManager({ api, categories, people, toast }: { api: Backend; categories: Category[]; people: Person[]; toast: Toast }) {
  const [editing, setEditing] = useState<{ id: number; name: string } | null>(null);
  const [adding, setAdding] = useState("");
  const count = (c: Category) => people.filter((p) => tr.lower(p.category) === tr.lower(c.name)).length;
  const fail = (e: unknown) => toast(errorText(e), true);

  const rename = async () => {
    if (!editing) return;
    try { await api.renameCategory(editing.id, editing.name.trim()); setEditing(null); } catch (e) { fail(e); }
  };
  const add = async () => {
    if (!adding.trim()) return;
    try { await api.addCategory(adding); setAdding(""); } catch (e) { fail(e); }
  };

  return (
    <div className="group">
      <h3>Kategoriler</h3>
      <p>Ana ekranda arama kutusunun yanında bu sırayla görünür. Silinen kategorideki kişiler ilk kategoriye taşınır.</p>
      <ul className="catlist">
        {categories.map((c, i) => (
          <li key={c.id}>
            <span className="order">
              <button className="btn icon" disabled={i === 0} title="Sola al" onClick={() => api.moveCategory(c.id, -1).catch(fail)}>↑</button>
              <button className="btn icon" disabled={i === categories.length - 1} title="Sağa al" onClick={() => api.moveCategory(c.id, 1).catch(fail)}>↓</button>
            </span>
            {editing?.id === c.id ? (
              <input className="input" autoFocus value={editing.name} maxLength={24} onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                onKeyDown={(e) => { if (e.key === "Enter") rename(); if (e.key === "Escape") { e.stopPropagation(); setEditing(null); } }} />
            ) : (
              <span className="catname">{c.name} <small>{count(c)} kişi</small></span>
            )}
            <span className="actions">
              {editing?.id === c.id ? (
                <>
                  <button className="btn small primary" onClick={rename}>Kaydet</button>
                  <button className="btn small" onClick={() => setEditing(null)}>Vazgeç</button>
                </>
              ) : (
                <>
                  <button className="btn small" onClick={() => setEditing({ id: c.id, name: c.name })}>Adını değiştir</button>
                  {categories.length > 1 && (
                    <ConfirmButton label="Sil" onConfirm={() => api.deleteCategory(c.id).then((m) => toast(m || `"${c.name}" silindi`), fail)} />
                  )}
                </>
              )}
            </span>
          </li>
        ))}
      </ul>
      <div className="row">
        <input className="input" placeholder="Yeni kategori adı (ör. Depo)" maxLength={24} value={adding}
          onChange={(e) => setAdding(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button className="btn" onClick={add}>+ Kategori ekle</button>
      </div>
    </div>
  );
}

export function PeopleScreen({ api, categories, people, toast }: { api: Backend; categories: Category[]; people: Person[]; toast: Toast }) {
  const [draft, setDraft] = useState<PersonDraft | null>(null);
  const [q, setQ] = useState("");
  const [picked, setCat] = useState<string | null>(null);
  const cat = picked !== null && categories.some((c) => c.name === picked) ? picked : null; // category may have been deleted
  const sorted = useMemo(() => [...people].sort((a, b) => tr.sort(a.name, b.name)), [people]);
  const needle = tr.lower(q.trim());
  const list = sorted.filter((p) => (cat === null || p.category === cat) && (!needle || tr.lower(p.name).includes(needle) || tr.lower(p.category).includes(needle)));
  const filters: [string | null, string, number][] = [
    [null, "Tümü", people.length],
    ...categories.map((c): [string, string, number] => [c.name, c.name, people.filter((p) => p.category === c.name).length]),
  ];
  const fail = (e: unknown) => toast(errorText(e), true);

  const save = async () => {
    if (!draft) return;
    if (!draft.name.trim()) return toast("İsim yazın", true);
    try {
      if (draft.id === null) {
        if (!draft.soundFile) return toast("Bu buton için bir ses dosyası seçin", true);
        await api.createPerson({ ...draft, soundFile: draft.soundFile });
        toast(`${draft.name.trim()} eklendi`);
      } else {
        await api.updatePerson(draft.id, draft);
        toast(`${draft.name.trim()} güncellendi`);
      }
      setDraft(null);
    } catch (e) { fail(e); }
  };

  const editorRef = useRef<HTMLDivElement>(null);
  const draftId = draft ? draft.id ?? 0 : null;
  useEffect(() => {
    if (draftId !== null) editorRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [draftId]);

  // Opens right where it was asked for: at the top for a new button, under the row for an edit.
  const editor = draft && (
    <div className="group editor" ref={editorRef}>
      <h3>{draft.id === null ? "Yeni anons butonu" : `Düzenle: ${people.find((p) => p.id === draft.id)?.name ?? ""}`}</h3>
      <div className="row">
        <Field label="İsim (butonda görünen)">
          <input className="input wide" autoFocus value={draft.name} maxLength={40} onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            onKeyDown={(e) => { if (e.key === "Enter") save(); if (e.key === "Escape") setDraft(null); }} />
        </Field>
        <Field label="Kategori">
          <select className="input" value={draft.category} onChange={(e) => setDraft({ ...draft, category: e.target.value })}>
            {categories.map((c) => <option key={c.id} value={c.name}>{c.name}</option>)}
          </select>
        </Field>
      </div>
      <Field label="Buton rengi"><ColorPicker value={draft.color} onChange={(c) => setDraft({ ...draft, color: c })} /></Field>
      <Field label="Anons sesi">
        <SoundPicker api={api} file={draft.soundFile} hint={draft.name} toast={toast} onChange={(f) => setDraft({ ...draft, soundFile: f })} />
      </Field>
      <div className="row">
        <button className="btn primary" onClick={save}>Kaydet</button>
        <button className="btn" onClick={() => setDraft(null)}>Vazgeç</button>
      </div>
    </div>
  );
  const edit = (p: Person) => setDraft(draft?.id === p.id ? null : { id: p.id, name: p.name, category: p.category, color: p.color, soundFile: p.soundFile });

  return (
    <section className="screen plate">
      <div className="screen-head">
        <h1>KİŞİLER</h1>
        <button className="btn primary" onClick={() => setDraft({ id: null, name: "", category: categories[0]?.name ?? "Personeller", color: null, soundFile: null })}>
          + Yeni anons butonu
        </button>
      </div>

      {draft && draft.id === null && editor}

      <CategoryManager api={api} categories={categories} people={people} toast={toast} />

      <div className="group">
        <div className="row spread">
          <h3>Anons butonları ({list.length === people.length ? people.length : `${list.length} / ${people.length}`})</h3>
          <input className="input" placeholder="Ara…" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <div className="tabs" role="tablist" aria-label="Kategoriye göre süz">
          {filters.map(([k, l, n]) => (
            <button key={k ?? "*"} className="tab" role="tab" aria-selected={cat === k} onClick={() => setCat(k)}>{tr.upper(l)}<span>{n}</span></button>
          ))}
        </div>
        <table className="table">
          <thead><tr><th>İsim</th><th>Kategori</th><th>Ses</th><th>Çağrı</th><th /></tr></thead>
          <tbody>
            {list.map((p) => (
              <Fragment key={p.id}>
              <tr className={draft?.id === p.id ? "editing" : ""}>
                <td>
                  <button className="name-btn" title="Düzenle" onClick={() => edit(p)}>
                    <span className="dot" style={{ background: p.color ? vivid(p.color) : "var(--dim)" }} /> {p.name}
                  </button>
                </td>
                <td>
                  <select className="input slim" value={p.category} onChange={(e) => api.setPersonCategory(p.id, e.target.value).catch(fail)}>
                    {categories.map((c) => <option key={c.id} value={c.name}>{c.name}</option>)}
                    {!categories.some((c) => c.name === p.category) && <option value={p.category}>{p.category}</option>}
                  </select>
                </td>
                <td className="filename">{p.soundFile ?? <span className="bad">ses yok</span>}</td>
                <td className="mono">{p.count}</td>
                <td className="actions">
                  {p.soundFile && <button className="btn small" onClick={() => api.previewSound(p.soundFile!).catch(fail)}>▶</button>}
                  <button className={`btn small ${draft?.id === p.id ? "on" : ""}`} onClick={() => edit(p)}>{draft?.id === p.id ? "Kapat" : "Düzenle"}</button>
                  <ConfirmButton label="Sil" onConfirm={() => api.deletePerson(p.id).then(() => toast(`${p.name} silindi`), fail)} />
                </td>
              </tr>
              {draft?.id === p.id && <tr className="edit-row"><td colSpan={5}>{editor}</td></tr>}
              </Fragment>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/* ---------------- Ayarlar ---------------- */

function BackupSection({ api, toast, onChanged }: { api: Backend; toast: Toast; onChanged: () => void }) {
  const [busy, setBusy] = useState<"" | "save" | "restore">("");
  const [report, setReport] = useState<RestoreReport | null>(null);
  const [autos, setAutos] = useState<BackupFile[]>([]);
  const fail = (e: unknown) => toast(errorText(e), true);
  const load = () => api.listBackups().then(setAutos, () => {});
  useEffect(() => { load(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const backupNow = async () => {
    setBusy("save");
    try {
      const path = await api.createBackup();
      if (path) toast(`Yedek kaydedildi: ${path}`);
    } catch (e) { fail(e); } finally { setBusy(""); }
  };
  const restore = async (path?: string) => {
    setBusy("restore");
    try {
      const r = await api.restoreBackup(path);
      if (r) { setReport(r); onChanged(); load(); }
    } catch (e) { fail(e); } finally { setBusy(""); }
  };

  return (
    <div className="group">
      <h3>Yedekleme</h3>
      <p>
        Yedek; kişileri, kategorileri, renkleri, ses kayıtlarını, molaları, geçmişi ve ayarları içerir.
        Geri yüklerken hem bu uygulamanın hem eski FABE Anons'un yedekleri kabul edilir.
        Geri yüklemeden önce mevcut veriler otomatik yedeklenir, yani hiçbir şey kaybolmaz.
      </p>
      <div className="row">
        <button className="btn primary" disabled={!!busy} onClick={backupNow}>{busy === "save" ? "Yedekleniyor…" : "Tüm verileri yedekle"}</button>
        <button className="btn" disabled={!!busy} onClick={() => restore()}>{busy === "restore" ? "Geri yükleniyor…" : "Yedekten geri yükle"}</button>
      </div>
      {report && (
        <p className="result">
          {report.kind === "eski" ? "Eski uygulamanın yedeği" : "Yedek"} geri yüklendi: {report.people} kişi, {report.sounds} ses, {report.breaks} mola.
          {report.missingSounds.length > 0 && ` Sesi bulunamayan: ${report.missingSounds.join(", ")}.`}
        </p>
      )}
      {autos.length > 0 && (
        <>
          <h3>Otomatik yedekler</h3>
          <table className="table">
            <tbody>
              {autos.slice(0, 6).map((b) => (
                <tr key={b.path}>
                  <td className="mono">{b.created}</td>
                  <td className="filename">{b.name.includes("guncelleme") ? "Güncelleme öncesi" : b.name.includes("geri-yukleme") ? "Geri yükleme öncesi" : "Otomatik"}</td>
                  <td className="dim">{(b.size / 1e6).toFixed(1)} MB</td>
                  <td className="actions">
                    <button className="btn small" onClick={() => api.showInFolder(b.path).catch(fail)}>Klasörde göster</button>
                    <ConfirmButton label="Geri yükle" className="btn small" onConfirm={() => restore(b.path)} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}

function NetworkSection({ api, settings, toast, onChanged }: { api: Backend; settings: Settings; toast: Toast; onChanged: () => void }) {
  const [mode, setMode] = useState<NetMode>(settings.netMode);
  const [ip, setIp] = useState(settings.netServerIp);
  const [port, setPort] = useState(String(settings.netPort));
  const [pc, setPc] = useState(settings.pcName);
  const [test, setTest] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [ips, setIps] = useState<string[]>([]);
  const [status, setStatus] = useState<ServerStatus | null>(null);
  useEffect(() => { api.localIps().then(setIps, () => {}); }, [api]);
  useEffect(() => {
    if (settings.netMode !== "sunucu") return;
    const load = () => api.netStatus().then(setStatus, () => {});
    load();
    const t = window.setInterval(load, 5000);
    return () => window.clearInterval(t);
  }, [api, settings.netMode]);

  const save = async () => {
    try {
      await api.setSetting("net_mode", mode);
      await api.setSetting("net_server_ip", ip.trim());
      await api.setSetting("net_port", port.trim());
      await api.setSetting("pc_name", pc.trim());
      onChanged();
      toast("Ağ ayarları kaydedildi");
    } catch (e) { toast(errorText(e), true); }
  };
  const runTest = async () => {
    setTesting(true); setTest(null);
    try { setTest({ ok: true, text: await api.testConnection(ip, +port) }); }
    catch (e) { setTest({ ok: false, text: errorText(e) }); }
    finally { setTesting(false); }
  };

  const modes: [NetMode, string, string][] = [
    ["yerel", "Yerel", "Sadece bu bilgisayar anons yapar"],
    ["sunucu", "Sunucu", "Hoparlöre bağlı bilgisayar; istemcilerden gelen anonsları çalar"],
    ["istemci", "İstemci", "Anonsları sunucuya gönderir"],
  ];

  return (
    <div className="group">
      <h3>Ağ ayarları</h3>
      <div className="segmented">
        {modes.map(([k, l, d]) => (
          <button key={k} className={mode === k ? "on" : ""} onClick={() => setMode(k)}><b>{l}</b><small>{d}</small></button>
        ))}
      </div>
      <div className="row">
        <Field label="Bu bilgisayarın adı (geçmişte görünür)"><input className="input" value={pc} maxLength={30} onChange={(e) => setPc(e.target.value)} /></Field>
        {mode === "istemci" && (
          <Field label="Sunucu IP adresi"><input className="input" placeholder="192.168.1.20" value={ip} onChange={(e) => setIp(e.target.value)} /></Field>
        )}
        {mode !== "yerel" && (
          <Field label="Port"><input className="input narrow" inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} /></Field>
        )}
      </div>
      {mode === "sunucu" && ips.length > 0 && (
        <p>İstemcilerde sunucu IP adresi olarak şunu yazın: <b className="mono">{ips.join(", ")}</b></p>
      )}
      {settings.netMode === "sunucu" && status && (
        <div className="server-status">
          {status.listening ? (
            <span className="result">● Sunucu çalışıyor, {status.port} portunu dinliyor</span>
          ) : (
            <span className="result err">● Sunucu çalışmıyor{status.error ? `: ${status.error}` : ""}</span>
          )}
          {status.clients.length > 0 ? (
            <ul className="list">
              {status.clients.map((c) => (
                <li key={c.ip}><span className="t">{c.lastSeen}</span><span className="what">{c.name}</span><span className="src">{c.ip}</span></li>
              ))}
            </ul>
          ) : <p>Henüz bağlanan istemci yok.</p>}
        </div>
      )}
      <div className="row">
        <button className="btn primary" onClick={save}>Kaydet</button>
        {mode === "istemci" && <button className="btn" disabled={testing} onClick={runTest}>{testing ? "Deneniyor…" : "Bağlantıyı test et"}</button>}
        {test && <span className={`result ${test.ok ? "" : "err"}`}>{test.text}</span>}
      </div>
      <p className="dim">
        İstemci modunda butonlara basınca anons sunucuya gider ve sunucunun hoparlöründen çalar. Sunucuda o ses yoksa
        dosya otomatik gönderilir, yani her bilgisayar kendi kişi listesini ve kayıtlarını kullanabilir. Sunucu ilk açıldığında
        Windows güvenlik duvarı izin isterse "İzin ver"e basın.
      </p>
    </div>
  );
}

export function UpdateSection({ api, settings, release, onRelease, toast }: {
  api: Backend; settings: Settings; release: Release | null; onRelease: (r: Release | null) => void; toast: Toast;
}) {
  const [checking, setChecking] = useState(false);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [checked, setChecked] = useState(false);
  useEffect(() => api.onUpdateProgress(setProgress), [api]);

  const check = async () => {
    setChecking(true);
    try { onRelease(await api.checkUpdate()); setChecked(true); } catch (e) { toast(errorText(e), true); } finally { setChecking(false); }
  };
  const install = async () => {
    if (!release) return;
    setProgress({ step: "Başlıyor…", done: 0, total: 0 });
    try { await api.installUpdate(release); } catch (e) { setProgress(null); toast(errorText(e), true); }
  };
  const pct = progress && progress.total ? Math.round((progress.done / progress.total) * 100) : null;

  return (
    <div className="group">
      <h3>Güncelleme</h3>
      <p>Kurulu sürüm: <b className="mono">{settings.version}</b></p>
      {release ? (
        <div className="update-box">
          <b>Yeni sürüm var: {release.version}</b>
          {release.notes && <p>{release.notes}</p>}
          {progress ? (
            <>
              <p className="result">{progress.step}{pct !== null && ` %${pct}`}</p>
              <div className="bar"><i style={{ width: `${pct ?? 0}%` }} /></div>
            </>
          ) : (
            <button className="btn primary" onClick={install}>{release.version} sürümüne güncelle</button>
          )}
          <p className="dim">Güncellemeden önce tüm veriler ve ayarlar otomatik yedeklenir. Uygulama kapanıp yeni sürümle yeniden açılır.</p>
        </div>
      ) : (
        <div className="row">
          <button className="btn" disabled={checking} onClick={check}>{checking ? "Kontrol ediliyor…" : "Güncellemeleri kontrol et"}</button>
          {checked && <span className="result">En son sürümü kullanıyorsunuz.</span>}
        </div>
      )}
    </div>
  );
}

const EMA_STATE: Record<TtsStatus["ema"], string> = {
  yok: "Bu kurulumda yok",
  kapali: "Kapalı (ilk anonsta açılır)",
  yukleniyor: "Yükleniyor…",
  hazir: "Hazır",
  hata: "Açılamadı",
};

function TtsSection({ api, settings, toast, onChanged }: { api: Backend; settings: Settings; toast: Toast; onChanged: () => void }) {
  const [status, setStatus] = useState<TtsStatus | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => api.ttsStatus().then((s) => alive && setStatus(s), () => {});
    load();
    const t = window.setInterval(load, 3000);
    return () => { alive = false; window.clearInterval(t); };
  }, [api]);
  const choose = (v: TtsEngineChoice) => api.setSetting("tts_engine", v).then(onChanged, (e) => toast(errorText(e), true));
  const choices: [TtsEngineChoice, string, string][] = [
    ["otomatik", "Otomatik", "ema-lightning, olmazsa Piper"],
    ["ema", "ema-lightning", "Yalnızca ana motor"],
    ["piper", "Piper", "Yalnızca yedek motor"],
  ];
  return (
    <div className="group">
      <h3>Canlı anons motoru</h3>
      <div className="segmented">
        {choices.map(([k, l, d]) => (
          <button key={k} className={settings.ttsEngine === k ? "on" : ""} onClick={() => choose(k)}><b>{l}</b><small>{d}</small></button>
        ))}
      </div>
      {status && (
        <dl className="engine-state">
          <dt>ema-lightning</dt><dd>{EMA_STATE[status.ema]}{status.emaError ? ` (${status.emaError})` : ""}</dd>
          <dt>Piper</dt><dd>{status.piper === "hazir" ? "Hazır" : "Bu kurulumda yok"}</dd>
          {status.lastEngine && <><dt>Son anons</dt><dd>{ENGINE_LABEL[status.lastEngine]}{status.lastNote ? ` · ${status.lastNote}` : ""}</dd></>}
        </dl>
      )}
      <p>Canlı anonslar da 2 kez okunur. Sorun olursa ayrıntılar veri klasöründeki hata.log ve tts.log dosyalarına yazılır.</p>
    </div>
  );
}

export function SettingsScreen({ api, settings, release, onRelease, toast, onChanged }: {
  api: Backend; settings: Settings; release: Release | null; onRelease: (r: Release | null) => void; toast: Toast; onChanged: () => void;
}) {
  const setChime = (on: boolean) => api.setSetting("chime", String(on)).then(onChanged, (e) => toast(errorText(e), true));
  return (
    <section className="screen plate">
      <h1>AYARLAR</h1>
      <UpdateSection api={api} settings={settings} release={release} onRelease={onRelease} toast={toast} />
      <div className="group">
        <h3>Anons</h3>
        <label className="toggle">
          <input type="checkbox" checked={settings.chime} onChange={(e) => setChime(e.target.checked)} />
          Anonstan önce gong çal
        </label>
        <p>İsimler her zaman 2 kez okunur. Anons sırasında diğer programların sesi %10'a kısılır, bitince eski seviyesine döner.</p>
      </div>
      <TtsSection api={api} settings={settings} toast={toast} onChanged={onChanged} />
      <BackupSection api={api} toast={toast} onChanged={onChanged} />
      <NetworkSection key={settings.netMode + settings.netServerIp + settings.netPort + settings.pcName} api={api} settings={settings} toast={toast} onChanged={onChanged} />
    </section>
  );
}
