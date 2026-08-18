# `#[hooks]` エントリの `&self` レシーバ設計検討

Status: design draft(実装なし・設計のみ)

Target: rshooks v0.2.x(採用する場合。§8 の導入タイミング判断を参照)

Last updated: 2026-08-18

Relates to: [MULTI_HOOK_STRUCT_DESIGN.md](./MULTI_HOOK_STRUCT_DESIGN.md)(r4 §5.5 / §5.7 の改訂提案)

## 0. 提案

現行 v0.2 では、`#[hooks]` struct で宣言した state / param へのアクセスは
**struct と同名の static** を経由する:

```rust
#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn deposit() -> i64 {
        let Ok(cfg) = Vault.config.get_or_default() else { /* ... */ };
        // ...
    }
}
```

これを `&self` レシーバで書けるようにする:

```rust
#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn deposit(&self) -> i64 {
        let Ok(cfg) = self.config.get_or_default() else { /* ... */ };
        // ...
    }
}
```

## 1. 結論(要約)

**採用を推奨する。形態は「任意の `&self`」(現行の self なし形式も引き続き合法)、
book の正典スタイルは `&self` とする。**

- **WCE / サイズ / ネストへの影響は実測ゼロ**(§5)。`#[inline(never)]` 境界を
  挟んだ場合でも self なし版と **wasm バイト一致**。コスト面の反対理由は存在しない。
- DX は明確に向上する(§4): `self.` による宣言フィールドの列挙・補完、
  struct リネーム耐性、ヘルパー合成の自然さ。
- r4 §5.7 の却下理由(「実体があるかのような誤解」)は再検証の結果、
  **論拠が逆転する**(§2): エントリを「唯一のインスタンス(static)のメソッド」と
  読ませる方が、現行の「struct 名を値として使う」形式より Rust として正直である。
- 必須化(全エントリに `&self` を強制)は**推奨しない**(§7)。宣言を持たない
  unit struct の最小形に儀式を足すだけであり、impl 外の自由関数からの
  アクセスには結局 static が必要なため、self なし形式を排除する利得がない。

## 2. 現行設計(r4 §5.7)の根拠の再検証

r4 §5.7 が self を却下した理由と、その再評価:

| r4 の論拠 | 再評価 |
|---|---|
| 「`&self` を許すと実体があるかのような誤解を生む」 | **実体は既にある**。r4 §5.5 が同名 static(`static Vault: Vault`)を導入した時点で、`Vault.deposits` という「値としてのアクセス」は存在している。`&self` はその同じ値を引数で受け取るだけで、新しい誤解を導入しない。むしろ「`Vault` という式が型なのか値なのか」という現行形式の分かりにくさ(Rust 初学者には unit struct 値 / static の値 namespace は非自明)を、「エントリは chain インスタンスのメソッド」という一貫した読みに置き換える |
| 「ラッパー生成が無駄に複雑になる」 | 実測で無視できる差(§6): wrapper の呼び出しが `Vault::deposit()` → `Vault::deposit(&Vault)` に変わるだけ。receiver 検出はエントリ形状検査(既存の `scan` が receiver 形状を既に認識している)の分岐 1 つ |
| 「`self` の誤用(`&mut self` 等)への診断が必要」 | どのみち必要(現行も self 全種を拒否する診断を持つ)。拒否対象が「self 全種」から「`&self` 以外の receiver」に変わるだけで、診断の実装量は同等 |

一方、r4 の懸念のうち**今も有効なもの**:

- **OO 的期待の助長**: `&self` があると「フィールドに実行時状態を持てる」
  「`&mut self` で書き換えられる」という期待が強まる。→ 対策は §6.2 の
  専用診断と book の明示(「フィールドは ZST ハンドル。ledger 状態はハンドル
  経由でのみ読み書きする」)。
- **2 形式併存による一貫性低下**(任意化する場合)。→ §7 で評価。

## 3. 意味論(採用する場合の確定案)

### 3.1 受理する receiver

| receiver | 扱い |
|---|---|
| なし(現行) | 引き続き合法 |
| `&self` | 合法(新規) |
| `self` / `mut self` / `&mut self` / `&'a self` / `self: T` 型注釈付き | **エラー**(専用診断。§6.2) |

`self`(値渡し)は ZST なので技術的には等価だが、**教えることを 1 つに絞る**ため
`&self` のみとする。

### 3.2 ラッパー生成

エントリ関数の receiver 有無を検出し、export ラッパーの呼び出しを切り替える:

```rust
// receiver なし(現行どおり)
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { super::Vault::deposit() }

// &self
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { super::Vault::deposit(&super::Vault) }
```

`&Vault` は**両方の struct 形で同じ式**が使える:

- 名前付きフィールド struct: `Vault` = 生成済み static(値 namespace)
- unit struct: `Vault` = unit 構築子の値

つまり unit struct のために static を追加生成する必要は**ない**(現行の
「unit struct は static を生成しない」ルール(r4 §5.5)は不変)。

### 3.3 cbak・ヘルパー

- `#[cbak(i)]` も同じ規則(receiver なし / `&self` の二択)。
- **annotated impl 内のヘルパー関数(属性なし)にも `&self` を許可する**。
  現状は self なしヘルパーのみ通過し、`&self` ヘルパーは拒否される
  (実測プローブで確認。エントリ用 receiver 検査がヘルパーにも及んでいる)。
  採用時にヘルパーの receiver 検査を緩和する(ヘルパーは `&self` /
  receiver なしの両方可。`&mut self` はヘルパーでも拒否 — ZST に対して
  無意味であり、可変性の誤解を招くだけのため)。
- impl 外の自由関数・別モジュールからのアクセスは従来どおり static
  (`Vault.deposits`)を使う。static は今後も公開インターフェースの一部である。

### 3.4 変わらないもの

- carrier(JSON にはエントリの fn 名しか載らず、receiver は現れない)
- rshooks-build(discovery / 選択ビルド / sidecar / テンプレート全て不変)
- ハンドシェイク・マーカー型・アクセサ API
- 診断カタログの他の項目(index 検証等)

## 4. DX 分析

### 4.1 メリット

1. **宣言の列挙・補完**: エントリ本文で `self.` と打つと、その chain が宣言する
   全 state / param が IDE に列挙される。「この Hook は何を読めるのか」が
   タイプ 1 つで見える。static 形式(`Vault.`)でも補完自体は効くが、
   struct 名を思い出して打つ必要があり、エントリごとに綴りが揺れる余地がある。
2. **リネーム耐性**: struct 名変更時にエントリ本文の書き換えが不要になる
   (rust-analyzer のリネームは static 参照も追うが、レビュー diff が小さいに
   越したことはない)。
3. **Rust としての自然さ**: 「chain 宣言オブジェクトのメソッドとしてのエントリ」
   という読みは、`impl` ブロックの本来の意味と一致する。r4 §4.3 が認めた
   「struct + impl という形の意味論的な嘘」が一段小さくなる。
4. **ヘルパー合成**: `self.helper_read()?` のような private メソッド分割が
   自然になる(現行はヘルパーも static 経由か引数渡し)。
5. **教材の一貫性**: book の説明が「`#[hooks]` は Vault のインスタンス
   (中身は空)を 1 つ用意し、エントリはそのメソッド」で完結する。

### 4.2 デメリット・リスク

1. **OO 的期待の助長**(§2)。`&mut self` を試みる利用者は必ず現れる。
   → 専用診断(§6.2)+ book の early warning で受け止める。実測ゼロコスト
   なので「動くのに遅い」類の罠はない。
2. **2 形式併存**(任意化の場合): 同一 codebase に `self.x` と `Vault.x` が
   混在し得る。→ book・examples は `&self` に統一し、self なし形式は
   「宣言を持たない最小 Hook」の形として位置づける。lint 的強制はしない。
3. **診断・fixture の改訂コスト**: 既存の「Hook entrypoints are stateless
   associated functions」診断の文言変更 + trybuild 更新(§6.3)。小。
4. **`Self` 型としての混乱は増えない**: 現行でも `Self::helper()` は書ける。
   receiver の有無は `Self` の可視性に影響しない。

## 5. WCE / サイズ / ネストへの影響(実測)

実際の rshooks-build パイプライン(examples と同一プロファイル、
discovery + 選択ビルド + clean/flatten/unnest/guard/validate)で、
同一ロジック(typed アクセサ約 6 呼び出し: param 読み + state read/modify/write ×2)を
6 形態でビルドし比較した。

| 形態 | WCE (hook) | サイズ (bytes) | 最大ネスト | 備考 |
|---|---:|---:|---:|---|
| V0: 現行形式(エントリ内に直書き、static 経由) | 630 | 1562 | 4 | 基準 |
| V1: `&self` メソッドに委譲(inline 指定なし) | 630 | 1561 | 4 | V0 と実質同一(差は rodata の 1 byte のみ。シンボル名差によるデータ配置ずれで、コードパス差ではない) |
| V0n: self なしヘルパー `#[inline(never)]` | 637 | 1574 | 4 | |
| V1n: `&self` ヘルパー `#[inline(never)]` | 637 | 1574 | 4 | **V0n と wasm バイト一致** |
| V2n: 3 分割 `#[inline(never)]`(self なし) | 684 | 1665 | 5 | |
| V2: 3 分割 `#[inline(never)]`(`&self`) | 684 | 1665 | 5 | **V2n と wasm バイト一致** |

読み取れること:

1. **`&self` 自体のコストはゼロ**。LLVM が自由に inline できる場合(V1)は
   完全に消え、`#[inline(never)]` 境界を挟んでも(V1n/V2)self なし版と
   バイト一致する。ZST への参照は最適化で完全に消滅し、flatten パスの
   引数 spill にも現れない。
2. コストが出るのは **`#[inline(never)]` 境界の個数**(V0→V0n: +7 WCE、
   1 境界→3 境界: +47 WCE、ネスト 4→5)であり、receiver の有無とは無関係。
   これは既知の関数境界コストで、本提案では増減しない。
3. したがって「`&self` はゼロコスト抽象である」と book に明記してよい
   (実測の裏付けあり)。

(補足: 80_governance の「typed アクセサ高密度によるネスト爆発」問題
(build-budget finding)は呼び出し**密度**の問題であり、receiver 形式とは
独立。`&self` の採否はこの問題を改善も悪化もさせない。)

## 6. 実装影響(採用する場合)

### 6.1 変更点一覧

| 箇所 | 変更 | 規模 |
|---|---|---|
| `hooks_impl.rs` エントリ形状検査 | receiver 検出を「拒否」から「なし / `&self` の二値」へ。ラッパー呼び出し式の分岐 | 小 |
| `hooks_impl.rs` ヘルパー分類 | 属性なし関数の `&self` を通過させる(`&mut self` は拒否) | 小 |
| 診断 | §6.2 の文言に差し替え | 小 |
| trybuild fixtures | §6.3 | 小 |
| book / examples | 正典スタイルを `&self` へ(採用判断とセットで) | 中(機械的) |
| decl / build / carrier | **変更なし** | — |

### 6.2 診断(案)

- `self` / `mut self` / 型注釈付き: 「use `&self` — hook entrypoints receive
  the chain declaration by shared reference (it is zero-sized)」
- `&mut self`: 「chain handles are zero-sized and immutable; ledger state is
  accessed through the handles, not by mutating the struct — use `&self`」
- 文言確定時に既存の「stateless associated functions」文言を置換。

### 6.3 trybuild

- 既存 fail: `hooks_self_receiver.rs`(self 全種拒否)→ `&self` は pass 側へ
  移動し、fail 側は `&mut self` / `mut self` / 値渡し `self` の 3 ケースに分割。
- pass 追加: `&self` エントリ + `&self` ヘルパー + cbak `&self` の混在ケース、
  self なしとの混在ケース(両形式併存が合法であることの固定)。

### 6.4 互換性

**任意化(推奨案)は完全に追加的(additive)**で、既存の v0.2 コードは
一切変更不要。semver 上も 0.2.x のマイナー追加として成立する。

## 7. 選択肢比較

| | O1: 現状維持 | **O2: 任意の `&self`(推奨)** | O3: `&self` 必須 |
|---|---|---|---|
| WCE | — | 影響ゼロ(実測) | 影響ゼロ(実測) |
| DX | static 名の綴り依存 | `self.` 補完・統一的な読み。2 形式併存が唯一の弱点 | 形式が 1 つで最も一貫 |
| 破壊性 | なし | **なし**(追加的) | 全エントリ書き換え(PR #53 マージ前なら実質コストゼロ、マージ後は破壊的) |
| 最小形(unit struct) | `fn main() -> i64` | 変わらず | `fn main(&self) -> i64` — 宣言のない Hook に儀式を追加 |
| impl 外アクセス | static | static(併存) | static(結局必要 = 「self だけの世界」は作れない) |
| 教育 | 「struct 名 = 値」の説明が必要 | 「エントリはメソッド」+「外からは static」 | 同左だが self なし形式の説明が不要 |

O3(必須化)を推奨しない決め手は 2 つ:
(a) impl 外(自由関数・他モジュール)からのアクセスに static が残る以上、
「アクセスは常に self 経由」という一貫性は**どのみち完成しない**。
(b) 宣言を持たない unit struct の最小形(book の最初の例)に `&self` を
書かせるのは、初学者に「この self には何があるのか(答え: 何もない)」を
最初に説明させる羽目になり、教育順序が悪化する。

## 8. 導入タイミング

- **推奨(O2)を採る場合**: 追加的変更なので PR #53 と切り離せる。
  マージ後に独立した small PR(macro + 診断 + fixtures + book の正典スタイル
  切り替え + examples の `&self` 化)として実装するのが最も低リスク。
- **もし O3(必須)を選ぶなら**: PR #53 がマージされる**前**に組み込むべき
  (マージ後では 2 度目の破壊的変更になる)。この場合は examples 16 クレートの
  再書き換えが発生する。

## 9. 未決事項

| ID | 論点 | 推奨 |
|---|---|---|
| S1 | examples を `&self` 正典スタイルへ一括移行するか、新規ページのみ `&self` にするか | 一括移行(book と examples の乖離を作らない) |
| S2 | `Vault.` static 直アクセスを book でどう位置づけるか | 「impl 外からのアクセス手段」として 1 箇所で説明 |
| S3 | ヘルパーの `&mut self` を将来許すか(ZST なので無害ではある) | 拒否を維持(可変性の誤解に対する教育的一線) |
| S4 | clippy 相当の style lint(self なしエントリへの nudge)を build 側 info 診断で出すか | 出さない(両形式とも正当。うるさい lint は DX を下げる) |

## 10. 参照

- [MULTI_HOOK_STRUCT_DESIGN.md](./MULTI_HOOK_STRUCT_DESIGN.md) r4 §4.3 / §5.5 / §5.7(本提案が改訂を提案する箇所)
- 実測プローブ: 同一ロジック 6 形態の WCE/サイズ/ネスト比較(§5 の表。
  スクラッチクレートによる実ビルドパイプライン測定、2026-08-18)
- `crates/rshooks/tests/ui/pass/hooks_impl_qualified_helpers.rs`(現行のヘルパー通過仕様)
