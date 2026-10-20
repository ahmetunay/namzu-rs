# 7.4 — Namzu-RS / Jcode deney protokolü ve sonuçları

Durum: **tamam — 2026-10-20**. Yeni cihaz girişi kullanıcı tarafından onaylandı;
iki ürünün kataloğunda GPT-6-Luna görüldü. İki puanlanmayan READY turu ile gerçek
erişim doğrulandı; ardından 20 seri canlı görev koşusu tamamlandı. Jcode'un 10
görev çıktısındaki model alanı ayrıca `gpt-6-luna` olarak doğrulandı.

## Sonuç

[Sayısal sonuçlar ve bütün satırlar](../bench/comparison-2026-10-20.json).
Tablodaki doğruluk, aşağıda açıklanan orakl düzeltmesinden sonra **20 çıktının
tamamının** yeniden kontrolüne dayanır. Süre/RSS/sayaçlar özgün canlı koşulardandır;
yeniden kontrol için model çalıştırılmadı.

| Ölçüm | Namzu-RS | Jcode |
| --- | --- | --- |
| Doğruluk: 5 görev × 2 tekrar | **10/10** | **10/10** |
| Toplam ajan süresi | 90.511 s | 228.891 s |
| Medyan ajan süresi | 8.664 s | 19.534 s |
| En yüksek ölçülen peak RSS | 14.79 MiB | 46.14 MiB |
| Medyan peak RSS | 14.60 MiB | 45.08 MiB |
| Bildirilen model/araç çağrısı | 44 / 34; 10/10 koşu | Bilinmiyor; 0/10 koşu |

On eşlenmiş başarılı görev/tekrar çiftindeki Jcode/Namzu süre oranının medyanı
**2.35**. Toplam süre oranı **2.53**; ölçülen maksimum RSS oranı **3.12**.
Bu küçük görev setinde Namzu daha kısa sürede ve daha düşük peak RSS ile aynı
doğruluğu elde etti. İki tekrar; genel ürün üstünlüğü, istatistiksel anlamlılık
veya Rust dilinin etkisi hakkında sonuç vermez. İki ürün de Rust'tır.

### Orakl düzeltmesi ve denetim izi

[Özgün rapor](../bench/comparison-original-2026-10-20.json) ilk checker ile
Namzu 9/10, Jcode 10/10 gösterdi. Tek ret, ikinci refactor turundaydı:

```python
from legacy import normalize

def normalize_many(texts):
    return [normalize(text) for text in texts]
```

Bu, verilen görev için geçerli delegasyondur. Eski checker aday modülü yükledikten
sonra `legacy.normalize` üzerine probe koyuyordu; önceden bağlanmış fonksiyon
importu probe'u görmüyordu. Bu yüzden checker bir doğru çözümü yanlış reddetti.

Düzeltme: davranış kontrollerinden sonra probe kurulur ve aday modül ayrı adla
yeniden yüklenir. Hem `import legacy` hem `from legacy import normalize` desteklenir;
doğru davranışı kopyalayıp delegasyon yapmayan kod hâlâ reddedilir. Özel Rust
regresyon testi her iki iddiayı gerçek Python süreçleriyle sınar. Görev metni,
başlangıç dosyaları veya aday çözümler değiştirilmedi.

Yeni checker ile **yalnızca reddedilen koşu değil, 20 kayıtlı çıktının hepsi**
yeniden değerlendirildi. Refactor'a ait dört orakl hash'i değişti; tek karar
`incorrect → passed` oldu. Diğer 19 karar değişmedi. Süre/RSS/çağrı sayıları,
program/prompt/input hash'leri ve sıra alanlarının bütün 20 koşuda aynen korunduğu
doğrulandı. Bu bir post-hoc orakl düzeltmesidir; ilk rapor silinmedi ve sayısal
raporun `audit` alanı eski/yeni hash'leri ve değişen kararı açıkça kaydeder.

Tekrar değerlendirme aracı aynı Rust SDK oraklını kullanır; ajan veya model başlatmaz:

```sh
cargo run --release --locked -p namzu-bench --example recheck -- \
  artifacts/comparison-suite.json artifacts/compare/KOSU_ID artifacts/yeni-audit
```

Özgün suite/report hash'lerini ve workspace yollarını ilk checker etkisinden
önce kontrol eder. Yeni özel dizine rapor/log yazar; mevcut raporu ezmez.

## Hazırlanan deney

- Namzu-RS: 7.3 kaynak commit'i `99fd14409143de4d90d6d73e5b08101bc86fb746`.
- Jcode: resmî **v0.98.1 (1c7426eb1)** Linux x86_64 release'i yeniden kuruldu.
  Arşiv SHA-256'sı GitHub release digest'iyle doğrulandı:
  `449a8ed50aa9ac75e6afe9b8e916814172ff7cc63913c7ae1f9abd5b1bc5323c`.
  Wrapper ve asıl `.bin` dosyası ignored artifacts içinde, kaynak deposunda değil.
- Ortak model **gpt-6-luna**, effort **low**. Native CLI effort seçimi ve
  yeni Jcode profilinin config/env low seçimi kullanıldı; fallback yapılmadı.
- Beş görev: slug, intervals, toposort, ledger, refactor. İki tekrar, her
  görev/ajan/tekrar için ayrı temiz workspace: **20 seri koşu**.
- Ajan başına **300 saniye**; bağımsız checker ayrıca en fazla 30 saniye.
- İlk tekrar Namzu → Jcode; ikinci tekrar Jcode → Namzu. Aynı prompt,
  başlangıç dosyaları ve host oraklları, ortak model ve effort seçimi.
- Her iki tarafta host dosya okuma/yazma/shell yetkisi. Namzu araçları
  `read_file,list_files,search_files,write_file,apply_patch,shell` ile sınırlandırılır;
  Jcode `bash,read,write,edit,apply_patch,glob,grep`. Bunlar eşit yetki düzeyi
  beyanıdır; araç uygulamaları ve sistem prompt'ları birebir aynı değildir.
- Jcode update/selfdev kapalı, ayrı socket; telemetri kapalı. Yeni bir özel
  Jcode profiline CLI köprüsüyle erişim/identity tokenı aktarılır; refresh grant
  kopyalanmaz. Auth içeriği veya gerçek anahtarlar suite'e/rapora konmaz.

Yerel suite: ignored `artifacts/comparison-suite.json`. `bench validate` geçti.
Sayısal rapordaki provenance; ölçülen Namzu executable, Jcode launcher ve asıl
binary SHA-256'larını saklar. Namzu binary'si orakl düzeltmesinden **önce** derlenmiş
7.3 sürümüdür; düzeltme sonrası derlenen binary için bu süreler iddia edilmez.
Geçmişteki üç görevlik canlı pilot bu deneye karıştırılmadı.

## Deneyi yeniden kurma

1. `namzu-rs auth status` ve `namzu-rs models` ile canlı girişi/modeli doğrula.
2. `auth link-jcode --directory YENI_OZEL_PROFIL` ile Jcode'u bağla; yalnızca bu
   profilin config.toml'ında `[provider] openai_reasoning_effort = "low"` seç.
3. Run ortamında `JCODE_HOME` yeni profil; `JCODE_OPENAI_REASONING_EFFORT=low`,
   `JCODE_OPENAI_TRANSPORT=https`, `JCODE_NO_TELEMETRY=1`. Bu ayarlar zaten
   suite'in env-name allowlist'ine dahil; değerler suite'e kaydedilmez.
4. İki ürünün modeli gerçekten seçebildiğini doğrula; fallback kullanma. Sonra
   hazırlanmış suite'i `bench run` ile sırayla çalıştır. Hatalı görevleri sonuçtan
   silme veya başarılı olana kadar sessiz tekrar yapma; her yeni deney ayrı rapor.
5. Tamamlanmış `report.json` dosyasını aşağıdaki araca ver; sayısal raporu ve
   karşılaştırma koşullarını inceleyip sonuçlarla bu belgeyi güncelle.

Yeni suite için `bench init --task-set eval --model gpt-6-luna --jcode JCODE_YOLU
--file YENI_SUITE` kullan. JSON'da `repeats: 2` seç; Namzu args listesine
`--effort low --telemetry=false --tool-allow
read_file,list_files,search_files,write_file,apply_patch,shell` seçeneklerini
ayrı argv elemanları olarak ekle. Jcode `inherit_env` listesine
`JCODE_OPENAI_REASONING_EFFORT` ve `JCODE_NO_TELEMETRY` adlarını ekle.
Yeni deney düzeltilmiş refactor checker'ını kullanır; hash'i özgün rapordan farklıdır.

## Rapor doğrulama ve özet

```sh
python3 -m unittest discover -s scripts -p test_compare_report.py -v
python3 scripts/compare_report.py artifacts/KOSU/report.json \
  --out artifacts/comparison-numeric.json
```

Araç model çağırmaz. Yalnızca tamamlanmış v2 **live**, seri, 300 saniyelik,
tam beş görev/iki ajan matrisi kabul eder. Tekrar sayısı çift ve en az ikidir;
ters ajan sırası, eşleşen prompt/input/oracle hash'leri, sabit program hash'leri
ve ortak izin beyanı zorunludur. Yanlış çözüm ve orakl hatası ayrı kalır.
Normal exit 0 ve doğru orakl olmadan `passed` kabul edilmez.

Çıktı; görev kimliği, tekrar/sıra, status, hash'ler, süre/RSS ve bilinen sayısal
çağrı sayılarının allowlist'idir. Raw ajan/checker çıktısı, yerel dosya yolları,
session veya auth kimliği kopyalanmaz. Çıktı **yeni 0600 dosya** olarak yazılır;
mevcut dosya ezilmez. Model kimliği ve sayısal sonuçlar yayımlanmadan incelenir.

Her ajan için toplam durumlar, tüm koşuların toplam/medyan süresi, yalnızca
geçenlerin medyan süresi ve ölçülen maksimum/medyan peak RSS üretilir. Eşlenmiş
hız oranı yalnızca **iki ajanın da geçtiği** görev/tekrar çiftlerinden hesaplanır:
Jcode süresi / Namzu süresi. Oran >1 o çiftlerde Namzu'nun daha kısa sürdüğünü
gösterir; başarısız bir hızlı çıkış hız kazancı sayılmaz.

Çağrı sayıları ajanın bildirdiği verilerdir. Her sayaç için bilinen koşu sayısı
ve bunların toplamı raporlanır; hiç bilinmiyorsa toplam **null** kalır. Kısmi
kapsama bütün deneyin toplam çağrısıymış gibi sunulmaz. Süreler checker süresini
içermez; RSS `wait4.ru_maxrss` ölçümüdür, eşzamanlı bütün süreçlerin toplam RAM/PSS'si
değildir. Jcode wrapper hash'i `.bin`/bağımlılıkların hash'i yerine geçmez.

Beş offline Python test grubu; eksik/fixture rapor reddi, dengesiz sıra, değişen
program/görev hash'i, redaksiyon, bilinmeyen sayaç ve yalnızca ortak başarıdaki
hız hesabını doğruladı. Sentetik test verileri gerçek model sonucu değildir.
Küçük örneklemden istatistiksel üstünlük veya programlama dilinin etkisi çıkarılmaz.

Son doğrulama: **652 Rust testi**, format, Clippy `-D warnings`, release workspace
build, 5 Python rapor testi ve 9 release eval kabul grubu. Özgün ajan ölçümleri
üstünde yeni inference yapılmadan 20/20 orakl yeniden kontrolü gerçekleştirildi.

## Ek kör görev — jobq

Kullanıcının yeni task paketinde iki ajan birer kez çalıştırıldı. Teslimler
checker gelmeden hash ile sabitlendi. Sonradan sağlanan bağımsız checker ile
iki ajan da 8/8 görünür + 31/31 gizli testi geçti; ilk süre/RSS kayıtları korundu.
[Ölçümler, kalibrasyon ve checker sonucu](JOBQ-BENCH.md).
