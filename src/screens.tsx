// Management screens: Mola, Geçmiş, Kişiler (+ kategoriler), Ayarlar (yedek, ağ, güncelleme).
import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  BUTTON_COLORS, errorText, tr,
  type Backend, type BackupFile, type BreakEntry, type CallEntry, type Category, type NetMode,
  type Person, type Release, type RestoreReport, type Settings, type UpdateProgress,
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

export function BreaksScreen({ api, breaks, toast }: { api: Backend; breaks: BreakEntry[]; toast: Toast }) {
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
      <p>Uygulama açıkken, buradaki her saatte seçilen ses dosyası kendiliğinden çalınır (müzik o sırada kısılır).</p>

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

export function HistoryScreen({ history }: { history: CallEntry[] }) {
  return (
    <section className="screen plate">
      <h1>GEÇMİŞ</h1>
      <p>Son 30 anons. Hangi bilgisayardan gönderildiği de görünür.</p>
      <table className="table">
        <thead><tr><th>Zaman</th><th>Anons</th><th>Kaynak</th></tr></thead>
        <tbody>
          {history.length ? history.map((h) => (
            <tr key={h.id}><td className="mono">{h.at.slice(0, 16)}</td><td>{h.kind === "tts" ? `Canlı: ${h.title}` : h.title}</td><td>{h.source}</td></tr>
          )) : <tr><td colSpan={3} className="dim">Henüz anons yok.</td></tr>}
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
        <button key={c} className={`swatch ${value?.toLowerCase() === c.toLowerCase() ? "on" : ""}`} style={{ background: c }} title={c} onClick={() => onChange(c)} />
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
  const sorted = useMemo(() => [...people].sort((a, b) => tr.sort(a.name, b.name)), [people]);
  const needle = tr.lower(q.trim());
  const list = sorted.filter((p) => !needle || tr.lower(p.name).includes(needle) || tr.lower(p.category).includes(needle));
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

  return (
    <section className="screen plate">
      <div className="screen-head">
        <h1>KİŞİLER</h1>
        <button className="btn primary" onClick={() => setDraft({ id: null, name: "", category: categories[0]?.name ?? "Personeller", color: null, soundFile: null })}>
          + Yeni anons butonu
        </button>
      </div>

      {draft && (
        <div className="group editor">
          <h3>{draft.id === null ? "Yeni anons butonu" : `Düzenle: ${people.find((p) => p.id === draft.id)?.name ?? ""}`}</h3>
          <div className="row">
            <Field label="İsim (butonda görünen)">
              <input className="input wide" autoFocus value={draft.name} maxLength={40} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
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
      )}

      <CategoryManager api={api} categories={categories} people={people} toast={toast} />

      <div className="group">
        <div className="row spread">
          <h3>Anons butonları ({people.length})</h3>
          <input className="input" placeholder="Ara…" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <table className="table">
          <thead><tr><th>İsim</th><th>Kategori</th><th>Ses</th><th>Çağrı</th><th /></tr></thead>
          <tbody>
            {list.map((p) => (
              <tr key={p.id}>
                <td><span className="dot" style={{ background: p.color ?? "var(--dim)" }} /> {p.name}</td>
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
                  <button className="btn small" onClick={() => setDraft({ id: p.id, name: p.name, category: p.category, color: p.color, soundFile: p.soundFile })}>Düzenle</button>
                  <ConfirmButton label="Sil" onConfirm={() => api.deletePerson(p.id).then(() => toast(`${p.name} silindi`), fail)} />
                </td>
              </tr>
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
  useEffect(() => { api.localIps().then(setIps, () => {}); }, [api]);

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
      <div className="row">
        <button className="btn primary" onClick={save}>Kaydet</button>
        {mode === "istemci" && <button className="btn" disabled={testing} onClick={runTest}>{testing ? "Deneniyor…" : "Bağlantıyı test et"}</button>}
        {test && <span className={`result ${test.ok ? "" : "err"}`}>{test.text}</span>}
      </div>
      <p className="dim">Not: Sunucu ve istemci arasında anons gönderme bir sonraki sürümde etkinleşiyor; ayarlar şimdiden kaydedilir.</p>
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
      <BackupSection api={api} toast={toast} onChanged={onChanged} />
      <NetworkSection key={settings.netMode + settings.netServerIp + settings.netPort + settings.pcName} api={api} settings={settings} toast={toast} onChanged={onChanged} />
    </section>
  );
}
