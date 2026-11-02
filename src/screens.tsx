// Secondary screens. Mola, Geçmiş and Kişiler are read-only for now; editing comes next.
import { useState } from "react";
import { errorText, tr, type Backend, type BreakEntry, type Category, type CallEntry, type ImportReport, type Person, type Settings } from "./api";

export function BreaksScreen({ breaks }: { breaks: BreakEntry[] }) {
  return (
    <section className="screen plate">
      <h1>MOLA</h1>
      <p>Bu saatlerde mola anonsu kendiliğinden çalınır. Ekleme ve düzenleme bir sonraki adımda geliyor.</p>
      <table className="table">
        <thead><tr><th>Saat</th><th>Açıklama</th><th>Ses</th></tr></thead>
        <tbody>
          {breaks.length ? breaks.map((b) => (
            <tr key={b.id}><td className="mono">{b.time}</td><td>{b.label}</td><td>{b.soundFile}</td></tr>
          )) : <tr><td colSpan={3}>Kayıtlı mola yok.</td></tr>}
        </tbody>
      </table>
    </section>
  );
}

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
          )) : <tr><td colSpan={3}>Henüz anons yok.</td></tr>}
        </tbody>
      </table>
    </section>
  );
}

export function PeopleScreen({ api, categories, people, toast }: {
  api: Backend; categories: Category[]; people: Person[]; toast: (m: string, err?: boolean) => void;
}) {
  const move = (p: Person, c: string) => api.setPersonCategory(p.id, c).catch((e) => toast(errorText(e), true));
  const sorted = [...people].sort((a, b) => tr.sort(a.name, b.name));
  return (
    <section className="screen plate">
      <h1>KİŞİLER</h1>
      <p>{people.length} kişi. Kategoriyi buradan değiştirebilirsiniz; yeni kategoriyi ana ekranda sekmelerin yanındaki + ile ekleyin. Yeni kişi ekleme, ses kaydı yükleme ve renk seçimi bir sonraki adımda geliyor.</p>
      <table className="table">
        <thead><tr><th>İsim</th><th>Kategori</th><th>Ses</th><th>Çağrı</th></tr></thead>
        <tbody>
          {sorted.map((p) => (
            <tr key={p.id}>
              <td><span style={{ color: p.color ?? undefined }}>■</span> {p.name}</td>
              <td>
                <select className="select" value={p.category} onChange={(e) => move(p, e.target.value)}>
                  {categories.map((c) => <option key={c.id} value={c.name}>{c.name}</option>)}
                  {!categories.some((c) => c.name === p.category) && <option value={p.category}>{p.category}</option>}
                </select>
              </td>
              <td>{p.soundFile ?? <span style={{ color: "var(--rec)" }}>yok</span>}</td>
              <td className="mono">{p.count}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

export function SettingsScreen({ api, settings, toast, onChanged }: {
  api: Backend; settings: Settings; toast: (m: string, err?: boolean) => void; onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<{ ok: ImportReport } | { err: string } | null>(null);

  const restore = async () => {
    setBusy(true);
    try {
      const r = await api.importOldBackup();
      if (r) { setReport({ ok: r }); onChanged(); }
    } catch (e) {
      setReport({ err: errorText(e) });
    } finally {
      setBusy(false);
    }
  };

  const setChime = (on: boolean) => api.setSetting("chime", String(on)).then(onChanged, (e) => toast(errorText(e), true));

  return (
    <section className="screen plate">
      <h1>AYARLAR</h1>
      <div className="group">
        <h3>Anons</h3>
        <label className="toggle">
          <input type="checkbox" checked={settings.chime} onChange={(e) => setChime(e.target.checked)} />
          Anonstan önce gong çal
        </label>
        <p>İsimler her zaman 2 kez okunur. Anons sırasında diğer programların sesi %10'a kısılır, bitince eski seviyesine döner.</p>
      </div>
      <div className="group">
        <h3>Eski uygulamadan aktarım</h3>
        <p>
          Eski FABE Anons'un "Tüm verileri yedekle" ile aldığı ZIP dosyasını seçin. Kişiler, renkler, ses kayıtları,
          çağrı sayıları, mola saatleri ve ayarlar aktarılır. Aynı yedeği tekrar yüklemek kopya oluşturmaz.
        </p>
        <div className="row">
          <button className="btn primary" disabled={busy} onClick={restore}>{busy ? "Aktarılıyor…" : "Eski yedekten geri yükle"}</button>
          {report && "ok" in report && (
            <span className="result">
              {report.ok.people} kişi, {report.ok.sounds} ses, {report.ok.breaks} mola aktarıldı.
              {report.ok.missingSounds.length > 0 && ` Sesi bulunamayan: ${report.ok.missingSounds.join(", ")}`}
            </span>
          )}
          {report && "err" in report && <span className="result err">{report.err}</span>}
        </div>
      </div>
    </section>
  );
}
