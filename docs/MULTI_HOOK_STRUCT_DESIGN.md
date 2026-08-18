# Multi-Hook Struct API 設計書 (v0.2.0)

Status: design draft(実装なし・設計のみ)

Target: rshooks v0.2.0(破壊的変更を許容)

Last updated: 2026-08-18

改訂履歴:

- r1: 初版
- r2: Codex (gpt-5.6-sol) レビュー 1 回目の指摘 26 件を反映
- r3: Codex レビュー 2 回目の指摘 24 件を反映
- r4: Codex レビュー 3 回目(最終)の指摘 17 件を反映。
  **critical だったハンドル表現の欠陥を「フィールド型へのマーカー型引数注入」で
  解決**(§5.4)。双方向ハンドシェイクの具体化(§5.1)、受理する item 形状表
  (§5.1)、トリガー省略の継承 caveat と `on = all` の追加(§5.3)、
  欠落判定の正規規則(§5.6)、BuildPlan の cargo metadata 解決・専用 target
  ディレクトリ(§7.1)、素の cargo/rustdoc 契約(§7.6)、Phase 再配分
  (§12–§13)、hex 大文字の正規化(§9)、他
- r5: エントリ関数の `&self` レシーバを受理するよう改訂(§5.7)。詳細は
  [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md) を参照。

## 0. 提案の原型(歴史的記録)

> **注意**: 以下は最初のアイデアスケッチの記録である。引数名・cbak の name 指定など
> ここに現れる構文は本設計の確定構文ではない。**規範となる骨格は §8 を参照**
> (属性文法の完全な BNF とアクセサ名は Phase 1 で確定する)。

トップレベルの `#[hook]` / `#[cbak]` 定義を廃止し、struct + impl ブロックへ移す。

```rust
pub struct Hook {
    // state 定義
    // hook_param 定義
    // otxn_param 定義
}

impl Hook {
    #[hook(0, name = "func1", onincoming = [Payment], canemit = [])]
    // metadata も定義可能
    pub fn func1(...) {}

    #[hook(1, "func1")]
    pub fn func2(...) {}

    #[cbak(0, "func1")]
    pub fn cbak(...) {}
}
```

ビルドすると index に対応する hook/cbak ペアごとに wasm が生成され、
インストール用の `SetHook` トランザクションテンプレートも生成される。
`name` は on-ledger の `HookName` を表す。

## 1. 確定事項

| # | 論点 | 決定 |
|---|---|---|
| 1 | 定義の単位 | struct + impl を「1 チェーン = 1 クレート」の一級市民とする。トップレベル `#[hook]` は v0.2 で廃止し一本化 |
| 2 | index の意味 | **チェーン位置(SetHook `Hooks` 配列の位置)をソースで直接指定する**。0..=9、歯抜け可 |
| 3 | index の記法 | 属性の**先頭位置引数**(`#[hook(0, ...)]`)。named 形式は提供しない。単一 Hook でも省略不可 |
| 4 | hook/cbak の対応 | cbak は `#[cbak(0)]` のように **index のみ**で対応付ける。name の再指定はしない |
| 5 | 単独定義 | hook のみの index は可。**cbak のみの index はコンパイルエラー**。hook が 1 つも無い struct はエラー |
| 6 | 外側マクロ名 | `#[hooks]`(struct と impl の両方に付ける) |
| 7 | ビルド戦略 | まず**案 A(discovery + index ごとの `--cfg` 再コンパイル)**を採用。案 B(1 回コンパイル + wasm 分割)が理想形であり、将来の最適化として置き換える(§7) |
| 8 | Gas Hook (HookApiVersion 1) | **対象外**。v0.2 は Guard 型 (api_version 0) のみ |
| 9 | manifest との関係 | manifest(docs/spec.md 等)は検討段階のため、**本設計では考慮しない** |
| 10 | パラメータ default | 属性の `default` は**実行時 fallback 式のみ**。SetHook テンプレートへの installed value 埋め込みは v0.2 では行わない(§5.6, §9.2) |
| 11 | テンプレートの意味論 | SetHook テンプレートは**占有位置パッチ**であり、チェーン全体の宣言的実現ではない。デフォルトは fail-closed(override なし)(§9) |
| 12 | トリガー省略 | 全省略は合法で「**installation override を置かない**」を意味する(新規 HookDefinition なら protocol 既定 = SetHook 以外の全 type で発火。既存 definition の再利用時はその値を継承)。**保証付きの全 type 発火は明示形 `on = all`** で書く(§5.3) |
| 13 | 記述的名前 | struct 属性に記述的 `name` は**置かない**(クレート識別は Cargo package name)。entry 属性の `name` は on-ledger `HookName` 専用 |
| 14 | 生成物メタ情報 | テンプレート JSON は protocol-shaped に保ち、生成情報は**別 sidecar**(`sethook.template.meta.json`)へ置く(§9.2) |
| 15 | ハンドル表現 | フィールド型はマクロが**フィールド固有のマーカー型を第 2 型引数として注入**する形で書き換える(`State<V>` → `State<V, __Marker>`)。span 契約の唯一の例外(§5.4) |
| 16 | 生成 hex | テンプレート・sidecar 中の hex(HookName / CreateCode / マスク / namespace / hash)は**大文字**で正規化する |

## 2. 背景: 現状の構造と、この提案が解く問題

### 2.1 現状 (v0.0.x)

- 1 クレート = 1 Hook。`#[hook]` / `#[cbak]` を自由関数に付けると
  `export_name = "hook"` / `"cbak"` のラッパーが生成される。
- HookOn / HookCanEmit / HookName / name / description は `metadata!` で
  エントリポイントとは**別の場所**に宣言する。トリガーは対称形
  (`HookOn`)・方向形(`IncomingHookOn`+`OutgoingHookOn`)・全省略の
  三形を受け付ける。
- state / パラメータは `hook_state!` / `hook_parameter!` / `otxn_parameter!` で宣言する。
- ビルドは cargo (wasm32v1-none) → rshooks-build 後処理
  (cleaner / flatten / unnest / guard / validator)で、1 つの wasm を出力する。
- SetHook の組み立ては利用者の責任。

### 2.2 現状の痛点

1. **チェーン(複数 Hook)をプロジェクトとして表現できない。**
   `80_reward` と `81_govern` のような「同じアカウントに載る関連 Hook 群」は
   別クレートになり、共有する state レイアウト(seat/member key、`V*` 投票 key)を
   **各クレートに複製**している。複製された宣言は静かに drift する。
2. **メタデータとエントリポイントが離れている。**
3. **hook と cbak の対応が暗黙。**
4. **チェーン位置が管理されない。** どの Hook を `Hooks` 配列の何番目に置くかは
   ソースにもビルド成果物にも現れない。
5. **デプロイ工程が手作業。** SetHook JSON の組み立ては利用者に委ねられている。

本提案は 1〜5 をまとめて解決する。特に 4 は「index をソースに書く」ことで
**チェーン内の自分が占有する位置構成をコードレビューの対象にする**という
設計判断である。

## 3. 何が良くなるか

### 3.1 共有 ABI の単一宣言(最大の利得)

同一 namespace / 同一アカウントを共有する Hook 群が、**state・パラメータの型宣言を
一箇所で共有**できる。key レイアウトの複製が消え、「govern が書き、reward が読む」
ような state の producer/consumer が同じ型を参照することをコンパイラが保証する。

これは単なる利便性ではなく安全性の改善である。state レイアウトの不一致は
実行時にしか現れず、on-ledger データを壊す。現状この保証は目視レビューしかない。

注意: 共有されるのは **Rust 上の型・レイアウト宣言(スキーマ)**である。
installed parameter の実値は Hook エントリ(index)ごとに ledger 上で独立であり、
宣言の共有は値の共有を意味しない(§5.4 末尾)。

### 3.2 占有位置構成のコード化

index がチェーン位置を直接表すため、「このプロジェクトがどの位置にどの Hook を
置くか」がソースコードに現れ、diff・レビュー・履歴管理の対象になる。
先行 Hook の accept/rollback が後続へ与える影響や、`hook_param_set` による
先行→後続の受け渡しといった**順序に意味がある設計**を、デプロイ手順書ではなく
コードとして表現できる。

(ソースが宣言するのは自分が占有する位置だけであり、アカウント上の
チェーン全体を宣言的に規定するものではない。§9.3)

### 3.3 メタデータの局所性

`on_incoming` / `can_emit` / `name` がエントリ関数に直接付くことで:

- 関数を読めばトリガー条件が分かる(レビュー性向上)
- `metadata!` とエントリの対応付けという暗黙知が消える
- 関数の追加・削除とメタデータの追加・削除が構文的に連動する

### 3.4 hook/cbak ペアの明示化と検証

index によるペアリングで、index 重複・対応の欠落などがコンパイル時に、
emit 能力と cbak の整合が build 時に検証可能になる。
**hook/cbak/emit 系の検証項目・重大度・実行フェーズの正規定義は §6.2(表)と
§6.3(真理値表)にある**(§6.2 はそれ以外の構文・形状診断も併載する)。

### 3.5 デプロイ成果物の自動生成

struct 全体を見れば「このクレートが占有する位置と各 Hook の設定」が静的に
分かるため:

- index ごとの wasm(それぞれ 64 KiB 上限に個別に収まる)
- index ごとの metadata sidecar
- 占有位置に対する SetHook トランザクション**テンプレート**(§9)+ 生成情報 sidecar

が 1 回のビルドコマンドで出せる。テンプレートは Account / HookNamespace を
プレースホルダーに持つ**提出前編集を前提とした雛形**であり、そのまま submit
できる完成品ではない(§9.3)。それでも「マスク・CreateCode・位置」を
手で組む作業が消えることは DX として大きい。

### 3.6 wasm サイズ戦略として合理的

コードを共有しつつ artifact を分割するので、共通ヘルパー(XFL 演算等)は各 wasm に
複製されるが、**各 wasm は自分のエントリから到達可能なコードだけ**を含む。
65,535 bytes 制限は Hook ごとに独立なので、チェーン全体としては実質的に上限が
10 倍に広がる。1 wasm に全部詰める方式より明確に有利。

## 4. 何が悪くなるか・リスク

### 4.1 単純ケースの体験悪化(最重要リスク)

現在の examples 01〜15 はすべて単一 Hook であり、最小例は

```rust
metadata! { name: "accept-all" }

#[hook]
fn my_hook() -> i64 { accept!(b"ok", 0) }
```

で済む。新形式の最小形(§8.4)は struct 宣言・impl ブロック・必須 index が
加わり、**正味 4〜6 行と概念 2 つ(struct の器、index)が増える**。
トリガー省略が引き続き合法(§5.3)なので最小形に `on` は不要だが、
初学者向けチュートリアルの第一印象がやや悪化することは率直に認める。

explicit index を単一 Hook にも要求するのは「2 個目を足すときに初めて 0 を
書き足す」という非対称を避けるためであり、一様性を初回コストより優先した
トレードオフである(book の最初の章で明示的に説明する)。

「トップレベル `#[hook]` を sugar として残す」案は、2 系統の定義方法が併存して
教材・実装が二重化するため採らない。v0.2 で一本化する。

### 4.2 チェーン位置の再配置はソース変更になる

index = チェーン位置なので、同じ Hook 群を別の位置構成で載せ替える場合は
ソースの index を書き換えて再ビルドすることになる。

これは意図した設計である(§3.2)。別位置への使い回しが必要なケースは、
生成された SetHook テンプレートの `Hooks` 配列を手で並べ替えることでも対応できる
(wasm 自体は位置に依存しない)。テンプレートは「ソース宣言どおりのデフォルト」
であり、最終的な提出前編集は妨げない。

### 4.3 「struct + impl」という形の意味論的な嘘

Rust の struct/impl は本来インスタンスとメソッドのためのもの。Hook エントリは
インスタンスを持たない静的なエクスポートであり、`&self` は存在し得ない。
また struct フィールドも実データを持たない(ZST マーカー)。つまりこの構文は
**名前空間としての struct の借用**であり、OO 的な期待(「self の状態」)を
持った開発者を裏切る可能性がある。

これは致命傷ではない(wasm_bindgen や pymethods など前例は多い)が、
ドキュメントで「struct はチェーン宣言の器であり、実行時実体はない」と
明示する必要がある。

### 4.4 マクロの複雑化と IDE 体験

impl ブロック全体を書き換える外側マクロは、メソッド単位の属性より
実装・保守コストが一段上がる。対策として次を**実装要件**とする:

- **span 保持契約**(§6.1): ユーザーが書いたメソッド本体・シグネチャ・
  doc コメント・無関係な属性は span を保持したまま原文どおり再出力し、
  消費した helper 属性だけを取り除く。文字列化からの再構築はしない。
  **唯一の例外**は `#[hooks]` struct のフィールド型で、マーカー型引数の注入
  (§5.4)のために書き換える。その場合もユーザーが書いた既存トークン
  (値型引数など)は span を保持して再利用する。
- **診断カタログ**(Phase 1 成果物): よくある誤り —
  外側 `#[hooks]` の片方欠落、現行(v0.0.x)マクロの残存、方向指定の片側欠落、
  方向マスクの一致、`required` + `default` の併用、entry への `#[cfg]`、
  `self` 付きエントリ、非対応の struct/impl 形状(§5.1)— のそれぞれについて、
  エラーメッセージ・primary span・help 文言を仕様化し、trybuild UI テストで
  固定する。
- rust-analyzer 上での補完・goto のスモーク確認を Phase 1 の完了条件に含める。

### 4.5 state/param 宣言の「フィールド化」は見た目ほど自然ではない

`hook_state!(Counter, CounterKey {...} => u64)` は key リテラル・key 形状・値型を
1 宣言に束ねている。これをフィールドにするには、フィールド型だけでは表現できない
情報(key のリテラル値、パラメータ名バイト列)を**フィールド属性**で補う必要がある。
理由: `&'static str` / バイト列の const generics は stable でないため、
`HookParam<"CFG", Config>` のような型レベル埋め込みはできない。
属性の内容は§5.4 のとおりフィールド固有のマーカー型へ落とし込む。

つまりフィールド化しても「型 + 属性」の 2 要素宣言になり、現行マクロと情報量は
同じである。得られるのは局所性と全体の見通しであって、記述量の削減ではない。

### 4.6 ビルドパイプラインの複雑化

1 クレート → N wasm は、cargo の「1 cdylib = 1 成果物」モデルから外れる。
BuildPlan(§7.1)・per-index 処理順序(§7.2)・成果物世代管理(§7.4)・
素の cargo との契約(§7.6)を仕様として固定することでリスクを限定する。

### 4.7 移行コスト

examples 15 本 + book + e2e + テンプレートがすべて書き換えになる。
v0.2.0 の破壊的変更として許容範囲だが、作業量としてはマクロ実装と同オーダーの
工数を見込むべき。**移行の正規手順と検収基準は §12(移行計画)に定める。**

## 5. 意味論の確定

### 5.1 struct の単位・配置・受理する形

- **1 crate につき Hook struct は 1 つ**(v0.2)。
- `#[hooks]` を付けた impl ブロックは **struct につき厳密に 1 つ**。
  属性なしの通常の impl ブロック(ヘルパー用)は自由に併存でき、
  annotated impl の中に helper 属性を持たない通常の関連関数を置くことも
  **許可**する(マクロは無変更で通す)。
- **受理する item 形状**(v0.2。これ以外は専用診断で reject):

  | 対象 | 受理 | 拒否 |
  |---|---|---|
  | struct | 非ジェネリックの unit struct(`struct X;`)/ 名前付きフィールド struct(空 `{}` 含む) | tuple struct、ジェネリクス・lifetime・where 句付き |
  | struct フィールド | 宣言属性(`#[state]`/`#[hook_param]`/`#[otxn_param]`)を**ちょうど 1 つ**持つフィールド | 属性なしフィールド、複数宣言属性 |
  | impl | 非ジェネリックの inherent impl(self 型は裸の struct 名) | trait impl、ジェネリック impl、修飾付き self 型 |
  | impl 内 item | 関連関数(エントリ/ヘルパー)、関連定数 | 関連型 |

  unit struct から名前付き struct への移行は「`;` をフィールドブロックへ
  置き換えるだけ」であることを book に明記する。
- **struct/impl の双方向ハンドシェイク**:
  - struct マクロは (a) フィールドごとのマーカー型とハンドル static(§5.4/§5.5)、
    (b) `impl Vault { #[doc(hidden)] pub const __RSHOOKS_STRUCT: () = (); }`、
    (c) `const _: () = { fn assert<T: HookChainImpl>() {} let _ = assert::<Vault>; };`
    相当のアサーション(impl 側の trait 実装を要求)を生成する。
  - impl マクロは (a) 生成コード内で `Self::__RSHOOKS_STRUCT` を参照
    (**struct 側が annotated であることを要求** — 素の struct に annotated impl
    を付けると未定義関連定数エラー)、(b) `#[doc(hidden)]` の内部 trait
    `HookChainImpl` の実装(struct 側アサーションの要求先)、
    (c) `impl Vault { #[doc(hidden)] pub const __RSHOOKS_IMPL: () = (); }` を
    生成する(annotated impl が 2 つあると**関連定数の重複定義エラー**で確実に
    衝突する。trait 実装の E0119 に依存しない)。
  - 内部 trait / 定数は `#[doc(hidden)]` の内部 API であり、ユーザーが手で
    実装・定義して偽装することは**サポート外(結果未定義)**と文書化する。
    ここは悪意あるコードからの防御機構ではない。
- **「1 crate 1 struct」の強制はリンカに委ねる**: struct マクロは固定名の
  `#[unsafe(no_mangle)]` シンボル(wasm ターゲットのみ、`#[doc(hidden)]`)を
  生成し、struct が 2 つあると重複シンボルのリンクエラーになる。リンクが
  失敗する以上 discovery は走らないため、これ以上の診断品質は提供しない
  (エラー文言の由来は book のトラブルシューティングに記載)。
- `#[hooks]` の struct と impl は**同一モジュール内に置くこと**を要件とする。
  生成される値バインディング(§5.5)の可視性は struct の可視性に、
  各フィールドハンドルの可視性はフィールドの可視性に従う。
- struct 名は自由。クレート識別は Cargo.toml の package name/version を使う。
  struct 属性は `#[hooks(description = "...")]` のみ(確定事項 #13)。
- エントリメソッドおよび宣言フィールドへの `#[cfg]` / `#[cfg_attr]` は
  **v0.2 では禁止**(マクロがエラーにする)。

### 5.2 index と name の意味

- `index` は **0..=9 の一意な整数**で、次の 2 つを同時に意味する。
  1. このクレートが生成する artifact(hook/cbak ペア)の識別子
  2. 生成される SetHook テンプレートにおける `Hooks` 配列の位置(= チェーン位置)
- 記法は属性の**先頭位置引数**: `#[hook(0, ...)]` / `#[cbak(0)]`。
- 歯抜け(0, 2 のみ等)は**許可**する。テンプレートの空き位置は `{"Hook": {}}`
  (位置維持の no-op)になる。`Hooks` 配列は **0..=最大宣言 index** ちょうどの
  長さで生成し、末尾に余分なエントリは付けない(§9.2)。
- `name` は on-ledger `HookName`(NamedHooks amendment)で **optional**。
  省略時は無名 Hook。長さ規則は protocol 規範に従う
  (注意: 現行 `metadata!` の「2..=8 Unicode scalar」という authoring 規則と、
  byte 長ベースの規則案が混在してきた経緯がある。**Phase 1 で vendored xahaud
  実装を規範として長さ検証を確定**し、authoring 独自規則は廃止する)。
  複数 index の name 共有はプロトコル上合法なので許可するが、build 時に
  info 診断を出す。
- **cbak は `#[cbak(0)]` のように index のみで対応付ける。** hook のみの index は
  可。**cbak のみの index はエラー**。1 つの index に cbak は最大 1 つ。
- **hook が 1 つも無い struct はコンパイルエラー**(§5.1 ハンドシェイク)。

### 5.3 per-hook 属性で宣言できるメタデータ

| 引数 | 対応 | 必須 |
|---|---|---|
| 先頭位置引数 `0..=9` | index(artifact ID / チェーン位置) | Yes |
| `name = "..."` | `HookName` | No |
| `on = all` / `on = [Tx, ...]` | 対称トリガー | No(下記) |
| `on_incoming = [..]` / `on_outgoing = [..]` | 方向指定トリガー。**必ずペアで書く**。両方向の集合が一致する場合は `on` を使うこと(エラー) | No(下記) |
| `can_emit = [Tx, ...]` | HookCanEmit(下記三値) | No |
| `description = "..."` | sidecar 用 | No |

**トリガーの宣言形**(現行 `metadata!` の三形 + 明示 catch-all):

| 宣言 | wire 上の出力 | 意味 |
|---|---|---|
| 全省略 | トリガーフィールドを出さない | **installation override を置かない**。新規 HookDefinition なら protocol 既定(SetHook を除く全 type で発火、将来の type にも追従)。**同一 wasm の HookDefinition が既に存在する場合は Install 扱いでその definition のトリガーを継承**するため、catch-all の保証ではない |
| `on = all` | `HookOn` の all-zero マスク(SetHook ビットのみ非発火) | **保証付き catch-all**。将来追加される type にも追従(列挙ではなくマスクで表現) |
| `on = [..]` | `HookOn`(64-hex マスク) | 記載 type のみで発火。`on = []` は「どの type でも発火しない」 |
| `on_incoming` + `on_outgoing` | `HookOnIncoming` + `HookOnOutgoing`(各 64-hex マスク、`HookOn` とは排他) | HookOnV2 の方向別発火 |

「全 type で発火」を type 名の列挙で再現してはならない(将来の type 追加に
追従できない)。省略(継承あり得る)と `on = all`(保証)の使い分けを book に
明記する。

**`can_emit` の三値意味論**:

| 宣言 | wire 上の意味 |
|---|---|
| 省略 | `HookCanEmit` フィールドを出さない = **installation override を置かない**。新規 HookDefinition なら制限なし(SetHook 含む全 type を emit 可)。既存 definition の再利用時はその値を継承。**「制限なしの保証」ではない** |
| `can_emit = []` | 全拒否マスクを設置(**deny-all**) |
| `can_emit = [Payment]` | 記載 type のみ許可する allowlist マスクを設置 |

命名は snake_case(`on_incoming` / `can_emit`)。
`HookApiVersion` は 0 固定であり、属性引数を設けない(Gas Hook は対象外)。

**amendment 依存**: `name` は NamedHooks、方向指定は HookOnV2、
`can_emit`(present)は HookCanEmit を要求する。導出した集合の扱いは §9.2。

### 5.4 struct フィールドの宣言形式とハンドル表現

フィールドは「マーカー付き ZST 型 + 属性」で宣言する。

```rust
#[hooks(description = "Deposit vault with sweep")]
pub struct Vault {
    /// アカウントごとの預入残高。
    #[state(key(prefix = b"B", field(account: AccountId)))]
    deposits: State<DepositValue>,

    /// 運用者が SetHook 時に設定する上限。
    #[hook_param(name = b"CFG", default = Config { max: xfl!(1000), lock: 10 })]
    config: HookParam<Config>,

    /// 呼び出しトランザクションが指定する命令。
    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,
}
```

**ハンドル表現(確定事項 #15)**: ユーザーが書くフィールド型
`State<DepositValue>` / `HookParam<Config>` は**宣言糖衣**である。
同じ値型のフィールドが 2 つあると(例: `State<DepositValue>` が 2 つ)
型だけでは受信側を区別できず、属性の key/name をメソッドディスパッチへ
結び付けられない。そこで struct マクロは:

1. フィールドごとに固有のマーカー ZST(例: `__VaultFieldDeposits`)を生成し、
   属性から導出した key/name 仕様(リテラル・形状・エンコード)を
   内部 trait(`KeySpec` / `NameSpec` 相当)の実装として与える
2. フィールド型を `State<DepositValue, __VaultFieldDeposits>` のように
   **マーカーを第 2 型引数として注入した型へ書き換える**
   (ユーザーが書いた第 1 型引数のトークンは span を保持して再利用)

これによりフィールドごとの一意な受信型が得られ、アクセサはマーカーの
trait 実装から key/name を静的に解決する。フィールド型の書き換えは
span 保持契約の**唯一の例外**として §4.4 に明記した。

- 生成される get/set の意味論、`FromBytes`/`ToBytes`(prefix/exact decode)、
  key エンコードの意味論は**現行実装から変更しない**。宣言の置き場所の変更であり、
  byte ABI の変更ではない。
- 既存の `hook_state!` / `hook_parameter!` / `otxn_parameter!` は v0.2 で削除。
  実装上は内部ロジック(shape parser、key encoder)をフィールド属性パーサーから
  呼び出す。
- **フィールド属性文法の網羅性**: 上記は代表例であり、正式文法は現行 3 マクロが
  受け付ける**全宣言形**(リテラル key(utf8/hex/bytes)、複合 key 形状、
  既存型参照、pairing 形、複合パラメータ名パターン — examples 12/81 の全形)を
  1:1 で表現できなければならない。Phase 1 で「現行宣言 → フィールド属性」の
  **正規移行表**(§12.1)を作成し、機械的書き換え可能性をもって文法の完全性を
  検収する。属性文法の BNF は Phase 1 で確定する。
- **共有されるのはスキーマであって値ではない**: struct の宣言はチェーン内の
  全 Hook が同じレイアウトを使うことを保証するだけである。installed parameter の
  実値は Hook エントリ(index)ごとに ledger 上で独立に設定・継承・消去される。
  例えば `config` を index 0 には `max=1000`、index 1 には `max=50` として
  インストールすることは正当である。per-index sidecar には全宣言が載るが、
  これは「共有スキーマの転記」であって「その Hook が使う宣言の列挙」ではない
  (§10 D2)。

### 5.5 生成される値バインディングと lint 契約

- **名前付きフィールドを持つ struct** に対しては、マクロが
  `static Vault: Vault`(struct 名と同名の static)を生成し、
  `Vault.deposits.get(&acct)` のように値としてアクセスする。
  型名と値名は別 namespace のため衝突しない。フィールドは全て ZST で
  `Sync` かつ const 構築可能なので static の要件は自明に満たされる。
- **unit struct** はフィールドを持たないため **static を生成しない**
  (unit 構築子と同名になり `E0428` で衝突するため)。空の名前付き struct
  (`struct X {}`)は static を生成してもよいが、アクセス対象が無いので
  どちらでも観測差はない。
- **lint 契約**: 生成コードは repository 標準の `-D warnings` ビルドを
  素通りしなければならない。小文字 static には scoped な
  `#[allow(non_upper_case_globals)]`、未使用になり得る生成物には必要最小限の
  `#[allow(dead_code)]` を付与する。生成する内部 item(ラッパー・マーカー・
  ハンドシェイク定数・carrier)はすべて `#[doc(hidden)]` とする。
  examples 全体を `-D warnings` でビルドする CI をこの契約の検収とする。

### 5.6 パラメータの presence とアクセサ

`required` / `default` は排他。アクセサは**欠落と不正データを区別**する:

| 宣言 | 追加されるアクセサ(名称は Phase 1 で確定する暫定) | 意味 |
|---|---|---|
| (常に) | `get() -> Result<Option<T>, Error>` | 欠落 = `Ok(None)`。**decode 失敗・host エラー = `Err`** |
| `default = <expr>` | `get_or_default() -> Result<T, Error>` | 欠落時のみ宣言式の値。decode 失敗は `Err` のまま |
| `required` | `get_required() -> Result<T, Error>` | 欠落も `Err`(欠落専用のエラー種別) |

- **欠落判定の正規規則**: 「欠落」は **decode 前の host API 戻り値
  (`DOESNT_EXIST`)によってのみ**判定する。decoder(`FromBytes`/`FixedRead`)が
  返したエラーは何であれ**再解釈しない**(decoder 由来の `DoesntExist` を
  欠落へ誤変換しない)。現行の typed ヘルパーが host 読み取りと decode を
  1 つの `Result` に畳んでいる箇所は、この規則を満たす形に内部を分離する。
  `get_required()` の欠落エラーは専用 variant(名称は Phase 1 で確定)とし、
  decode エラーと判別可能にする。
- 基本形 `get()` は宣言モードに依らず同一シグネチャで常に提供する。
- `default = <expr>` は**実行時の compiled fallback 式**である。
  任意の Rust 式はマクロ展開時にバイト列へ評価できないため、
  **`default` の値は SetHook テンプレートへは載らない**(§9.2)。
  encoded default を成果物へ運ぶ仕組みは将来課題(§10 D5)。
- 現行 `hook_parameter!` / `otxn_parameter!` が生成する API との対応表を
  Phase 1 で作成して名称・シグネチャを確定する(§12.1)。現行 API との
  意味差(特に「decode 失敗時に fallback を適用しない」への変更が生じる場合)は
  移行表に明記する。

### 5.7 エントリ関数のシグネチャ

impl 内のエントリは **`self` を取らない関連関数**とし、シグネチャは現行と同じ
`fn() -> i64`(cbak も同様)。誤って `self` を書いた場合は
「Hook entrypoints are stateless associated functions」と専用診断を出す(§4.4)。

> **r5 改訂**: 上記は初版(r4)時点の記述。r5 で `&self` レシーバ
> (`fn(&self) -> i64`)も受理するよう改訂された — 詳細・意味論・診断文言は
> [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md) を参照。

## 6. 実装上の技術的検討

### 6.1 マクロ構成: 外側 `#[hooks]` + 内側 inert 属性

メソッド単位の属性マクロでは index 重複検出・cbak 対応検証・チェーン全体の
メタデータ収集ができないため、wasm_bindgen / pymethods と同じく
**impl ブロックに `#[hooks]` を付け、内側の `#[hook(...)]` / `#[cbak(...)]` は
外側マクロが消費する inert 属性**とする。struct 側も `#[hooks(...)]` を付ける。
struct と impl は同一モジュール内で、§5.1 の双方向ハンドシェイクにより結ばれる。
struct マクロと impl マクロは互いの展開を見られないため、残りの整合検証
(フィールド参照の実在など)は通常の型検査に委ねる。

展開時に生成するもの:

1. 各エントリの extern ラッパー
   `#[unsafe(export_name = "__rshooks_hook_3")] extern "C" fn ...`
   (選択ビルド時は該当 index のみ `hook` / `cbak` 名になる。§7)
2. index → メタデータ表の carrier(現行 `metadata!` carrier の複数 index 対応版)
3. エラー級のクロスチェック診断(§6.2)
4. フィールドごとのマーカー型と書き換え済みフィールド型(§5.4)
5. struct/impl 双方向ハンドシェイク(trait 実装・関連定数・アサーション。§5.1)
6. 「1 crate 1 struct」検出用の固定名リンクシンボル(§5.1)

**span 保持契約**: ユーザーの書いたトークンは原文 span のまま再出力し、
消費した helper 属性のみ除去する。唯一の例外はフィールド型への
マーカー注入(§5.4)。文字列化からの再構築はしない。

### 6.2 検証の置き場所

| 検証 | 場所 | 種別 |
|---|---|---|
| index 重複・範囲(0..=9)・cbak 対応・hook ゼロ(ブロック内)・トリガー形式の排他・方向指定の片側欠落・方向集合の一致・`required`+`default` 併用 | マクロ展開時 | error |
| 非対応の struct/impl 形状(§5.1 の表)・属性なしフィールド・`self` 付きエントリ | マクロ展開時 | error |
| annotated impl の欠落 / annotated struct の欠落 / annotated impl の重複 | 型検査(双方向ハンドシェイク §5.1) | error |
| 現行(v0.0.x)マクロの残存(`metadata!` 等) | 名前解決(v0.2 で削除済みのため未解決エラー)+ book の移行章で案内 | error |
| トランザクション名の解決(TRANSACTION_TYPES) | マクロ展開時(vendored 表) | error |
| `#[cfg]`/`#[cfg_attr]` の entry/フィールドへの使用 | マクロ展開時 | error |
| 複数チェーン struct | リンク(シンボル衝突) | error |
| `HookName` 長さ規則(Phase 1 確定の protocol 規範) | マクロ展開時 | error |
| cbak 宣言 vs `cbak` export の実在(index ごとの選択ビルド後・クリーニング前後) | rshooks-build | error |
| emit / can_emit / cbak の整合(§6.3) | rshooks-build(index ごとの選択ビルド wasm に対して) | §6.3 のとおり |
| `HookName` の重複共有 | rshooks-build | info |
| 64 KiB 制限・guard・validator | rshooks-build(wasm ごと、現行と同じ) | error |

**stable Rust の proc macro は `compile_error!` によるハードエラーしか
確実に出せない**ため、warning / info 級はすべて rshooks-build 側で報告する。

### 6.3 emit / can_emit / cbak 整合の正規真理値表

到達可能な `emit` の検出は **index ごとの選択ビルド後の wasm** に対して行う
(discovery ビルドは全エントリを含み、per-index の到達性を持たないため)。
「`emit` import が最終 wasm に残っている」ことを「emit を使う」の判定とする。

| `can_emit` 宣言 | `emit` 使用(検出) | cbak 宣言 | 判定 |
|---|---|---|---|
| 省略(override なし) | あり | あり | OK |
| 省略 | あり | なし | warning(emit するのに cbak が無い) |
| 省略 | なし | あり | warning(emit しないのに cbak がある) |
| 省略 | なし | なし | OK |
| `[]`(deny-all) | あり | — | warning(emit は実行時に必ず失敗する) |
| `[]` | なし | あり | warning(emit も許可も無いのに cbak がある) |
| `[]` | なし | なし | OK |
| 非空 allowlist | あり | あり | OK |
| 非空 allowlist | あり | なし | warning |
| 非空 allowlist | なし | — | warning(宣言が未使用) |

## 7. ビルド戦略: 1 クレート → N wasm

### 案 A: discovery ビルド + index ごとの `--cfg` 再コンパイル(v0.2 で採用)

#### 7.1 BuildPlan(全呼び出しの固定)

オーケストレータ(rshooks-build / xtask)は最初に**不変の BuildPlan** を構築し、
**discovery と全選択ビルドの両方**に適用する。BuildPlan は少なくとも次を固定する:

- `cargo metadata` により解決した package ID・workspace root・
  **正規の lockfile パス**(workspace member は workspace の lock を使う。
  examples workspace のように複数 lockfile が存在するリポジトリでの
  取り違えを防ぐ)
- lockfile の存在(無ければ生成してから開始)と digest。**各呼び出しの前後で
  digest を再検証**し、途中変化はエラー
- 完全な argv(`cargo rustc --release --target wasm32v1-none --locked
  -p <package-id> --crate-type cdylib` + `--cfg` / `--check-cfg`)、
  feature 集合、profile、incremental の無効化
- toolchain(rustc/cargo バージョン)、canonical な cwd、
  関与する Cargo 設定ファイル(`.cargo/config.toml`)の内容 digest
- 環境変数の allowlist と各値(それ以外は伝播させない)
- **本オーケストレーション実行専用の target ディレクトリ**
  (`target/rshooks-build/<run or package>` 等)。discovery と選択ビルドは
  この専用ディレクトリを共有してキャッシュを効かせるが、ユーザーの通常の
  `cargo build` とは分離し、**cargo の artifact 出力を読むまでの間に別プロセスが
  上書きする TOCTOU を構造的に排除**する。オーケストレーションロックは
  コンパイル開始からステージング完了まで保持する
- `--check-cfg=cfg(rshooks_entry, values("0","1",...,"9"))` は**オーケストレータが
  毎回の呼び出しに自分で渡す**。値域は discovery 前でも既知の全定義域
  0..=9 で固定する(discovery 結果に依存させない)

cfg 名 `rshooks_entry` は予約とする。ユーザーコードが `cfg!(rshooks_entry)` を
参照して index ごとに挙動を変えることは**機械的に検出できない**ため、
「サポート外(結果は未定義)」という文書化された契約とする。

#### 7.2 per-index 処理順序(正規)

discovery の後、**index ごとに次を完了してから次の index へ進む**
(全体を通じて §7.1 のロックを保持):

1. BuildPlan + `--cfg 'rshooks_entry="<i>"'` でコンパイル
2. **直ちに** raw wasm バイト列をステージング領域へ読み取る
3. raw wasm から carrier を抽出し、§7.3 の整合検証を行う
   (**cleaner は carrier を除去するので、抽出は必ずクリーニング前**)
4. cbak 宣言 vs export の照合(クリーニング前の export 表に対して)
5. 既存 rshooks-build パイプライン(cleaner/flatten/unnest/guard/validator)
6. 最終 wasm の export が正確に `hook`(+宣言時 `cbak`)であることを確認し、
   ステージングへ確定

#### 7.3 discovery / 選択ビルドの整合検証(CanonicalRecord)

- 各選択ビルドの carrier に **`CanonicalRecordV1`** を含め、discovery のものと
  比較する。V1 に含める内容(最小):
  - schema バージョンタグ
  - index 集合(数値昇順)
  - index ごと: エントリ関数名、cbak の有無、`name`、トリガー宣言
    (**省略/`all`/`[]`/列挙・対称/方向を判別可能な形**)、`can_emit`
    (**省略と `[]` を判別可能な形**)、description
  - struct レベル: description、共有スキーマ(state/param 宣言)の正規化表現
  - `default` は**式のトークン列の正規化文字列**として含める(値は評価しない)
- 直列化はバージョン付き canonical byte 列(固定フィールド順、集合の正規順序、
  UTF-8、absent/empty の区別を保持)とし、digest は SHA-256。
  正確なバイトレイアウトは **Phase 2 開始時に確定**する(マクロ着手は妨げない)。
- 不一致時は bare digest ではなく**構造的差分**(どの index のどのフィールドか)
  と両ビルドのコンテキストを報告する。
- この検証が保証するのは**宣言メタデータの一致のみ**である。build script や
  環境依存マクロによるコード差分は BuildPlan の固定(§7.1)で抑えるが、
  完全な検出は保証しない(その旨を文書化する)。

#### 7.4 成果物世代管理(原子性)

- 出力は世代ディレクトリ `gen-<n>/` に書き、その Phase の**公開成果物一式**
  (Phase 2: wasm + per-index sidecar / Phase 3 以降: + テンプレート + meta
  sidecar)が揃って検証を通った後に `current` シンボリックリンクを付け替える。
- **消費者は `current` を 1 回だけ解決し、以後は解決先の不変な `gen-<n>/`
  パスを使うこと**(複数パスを `current/...` 経由で別々に開くと世代をまたぐ
  恐れがある)。この規約は成果物ディレクトリの README に明記する。
- ステージングは出力先と同一ファイルシステム上に置く。同時ビルドは
  ロックファイルで排他する。失敗時は `current` に触れない(前世代を保全)。
  古い世代は既定数(例: 2)を残して成功時に掃除し、掃除は解決済み `gen-<n>` を
  使用中の消費者と衝突し得ることを文書化する(即時削除しない猶予をおく)。

長所: 後処理パイプラインが「1 wasm = hook + optional cbak」という現行前提の
まま使える。LLVM の DCE でデータセグメント含め最小化される。
短所: コンパイル回数 N+1(依存クレートはキャッシュされるので leaf crate のみ)。

### 案 B: 1 回コンパイル + wasm 分割パス(理想形・将来の最適化)

suffix 付き export を全部含む 1 つの wasm を作り、split パスが index ごとに
「対象 export を `hook`/`cbak` に改名 → 他を削除 → DCE → 既存パイプライン」を
行う。コンパイル 1 回で最短、discovery と成果物が同一物になり §7.3 も不要に
なるが、wasm 手術(関数・テーブル・データセグメントの到達解析)の新規実装が
必要で、バグると全 Hook に波及する。

**決定: 理想形は案 B だが、v0.2 はシンプルさと既存パイプライン再利用を優先して
案 A を採用する。**

A→B の**同等性の定義**(§10 D4): バイト同一性・HookHash・サイズ・WCE は
**一致を期待しない**。要求するのは (1) index → (hook, cbak) 対応と全
deployment メタデータの一致、(2) 各 index の validator 通過、(3) index ごとの
差分実行テスト(同一入力に対する e2e での accept/rollback・戻り値・
host 呼び出し列の一致)。

(案 C: shim クレート生成は利点がなく不採用。)

### 7.5 成果物の命名

```
target/rshooks/<crate-name>/
  current -> gen-3/
  gen-3/
    0.deposit.wasm              # <index>.<fn名>.wasm
    0.deposit.metadata.json     # 現行 sidecar の per-index 版(共有スキーマ転記を含む)
    1.sweep.wasm
    1.sweep.metadata.json
    sethook.template.json       # 占有位置パッチ(§9)
    sethook.template.meta.json  # 生成情報 sidecar(§9.2)
```

### 7.6 素の cargo / rustdoc との契約

オーケストレータを介さない直接の cargo 実行についても挙動を規範化する:

- `cargo check` / `cargo doc` / docs.rs: **サポートする**(コンパイル可能で
  警告なし)。生成 item はすべて `#[doc(hidden)]` であり(§5.5)、rustdoc には
  ユーザーの公開 API だけが現れる。
- 素の `cargo build`: コンパイルは成功するが、成果物は suffix 付き export のみを
  持つ **discovery 相当の非デプロイ品**である(exact な `hook` export を
  持たないため、誤ってそのままインストールすることはできない —
  これは安全側の性質として意図的に維持する)。インストール可能な wasm は
  オーケストレータ経由でのみ生成される。この区別を book に明記する。
- 生成コード内の `cfg(rshooks_entry = ...)` 使用箇所は、`--check-cfg` を渡さない
  素の cargo でも `unexpected_cfgs` 警告を出さないよう、生成 item に scoped な
  `#[allow(unexpected_cfgs)]` を付与する(lint 契約 §5.5 の一部)。
- 素の check / build / doc の三通りに対する契約テストを Phase 1 に含める。

## 8. 構文(規範となる骨格)

> 属性文法の完全な BNF とアクセサ名は Phase 1 で確定する(§5.4, §5.6)。
> 以下の例のアクセサ名は暫定である。骨格(struct/impl、index、属性引数の
> 語彙と意味論)は本書で確定する。

### 8.1 マルチ Hook の例

```rust
#![no_std]
use rshooks::*;

#[hooks(description = "Deposit vault with sweep")]
pub struct Vault {
    #[state(key(prefix = b"B", field(account: AccountId)))]
    deposits: State<DepositValue>,

    #[hook_param(name = b"CFG", default = Config { max: xfl!(1000), lock: 10 })]
    config: HookParam<Config>,

    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,
}

#[hooks]
impl Vault {
    /// 入金を記録する。
    #[hook(0, name = "deposit", on_incoming = [Payment], on_outgoing = [], can_emit = [])]
    fn deposit() -> i64 {
        // 欠落時は default 式の値、decode 失敗は Err(§5.6)
        let Ok(cfg) = Vault.config.get_or_default() else {
            rollback!(b"vault: bad CFG", 1);
        };
        // ...
        accept!(b"deposited", 0)
    }

    /// 残高を回収して送金する。
    #[hook(1, name = "sweep", on = [Invoke], can_emit = [Payment])]
    fn sweep() -> i64 { /* ... */ }

    #[cbak(1)]
    fn sweep_cbak() -> i64 { accept!() }
}
```

### 8.2 hook_errors! / txn_template! との関係

`hook_errors!` と `txn_template!` は Hook 横断で共有可能な独立宣言なので、
v0.2 でも**トップレベルのまま変更しない**。

### 8.3 guard との関係

エントリの形(export ラッパー + 内部 fn)は現行 `#[hook]` の生成物と同じであり、
guard 挿入・検査(`_g` import、WCE 計算)は index ごとの wasm 単位で
従来どおり動く。影響なし。

### 8.4 単一 Hook の最小形

```rust
#[hooks]
pub struct MyHook;

#[hooks]
impl MyHook {
    #[hook(0)]  // トリガー省略 = installation override なし(§5.3)
    fn main() -> i64 {
        accept!(b"ok", 0)
    }
}
```

現行最小形(`metadata!` + `#[hook]` fn)との差は正味 4〜6 行(§4.1 の
評価を参照)。unit struct を許し、state/param が無ければフィールドは不要
(この場合 static は生成されない。§5.5)。index は単一でも省略不可
(確定事項 #3、根拠は §4.1)。

## 9. SetHook トランザクションテンプレート生成

### 9.1 生成物

`sethook.template.json`(占有位置パッチ。**protocol-shaped な編集用 JSON**:
フィールド構成はプロトコルどおりだが、プレースホルダーの置換と検証を経て
初めて valid なトランザクションになる):

```json
{
  "TransactionType": "SetHook",
  "Account": "<ACCOUNT>",
  "Hooks": [
    { "Hook": {
        "CreateCode": "<hex of 0.deposit.wasm>",
        "HookOnIncoming": "<64-hex mask (Payment)>",
        "HookOnOutgoing": "<64-hex deny-all mask>",
        "HookCanEmit": "<64-hex deny-all mask>",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "6465706F736974"
    } },
    { "Hook": {
        "CreateCode": "<hex of 1.sweep.wasm>",
        "HookOn": "<64-hex mask (Invoke)>",
        "HookCanEmit": "<64-hex allowlist mask (Payment)>",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "7377656570"
    } }
  ]
}
```

`sethook.template.meta.json`(生成情報 sidecar、submit 対象外):

```json
{
  "crate": "vault",
  "version": "0.2.0",
  "generated_at": "2026-08-18T09:00:00Z",
  "hook_hashes": { "0": "<sha512half>", "1": "<sha512half>" },
  "positions": { "declared": [0, 1], "gaps": [], "untouched_beyond": 2 },
  "required_amendments": ["Hooks", "NamedHooks", "HookOnV2", "HookCanEmit"]
}
```

マスク値は実際の生成物では 64 桁 hex の完全な値になる(HookOn は active-low +
SetHook ビット特例、HookCanEmit の deny-all は「全ビット 1、ただし SetHook
ビットのみ 0」であり、単純な all-F ではない。導出は既存実装を再利用)。
**生成する hex はすべて大文字で正規化する**(確定事項 #16)。

### 9.2 生成規則

- **トリガーの写像**(§5.3 の宣言形をそのまま wire へ):
  全省略 → トリガーフィールドなし / `on = all` → all-zero `HookOn` マスク /
  `on = [..]` → `HookOn` / 方向指定 → `HookOnIncoming` + `HookOnOutgoing`
  (`HookOn` とは排他)。
- `Hooks[i]` は index i の Hook。歯抜け index は `{"Hook": {}}`(位置維持の
  no-op)。配列長は **0..=最大宣言 index** ちょうど。
- `can_emit` 省略時は `HookCanEmit` フィールドを出さない(§5.3 の三値を
  wire までそのまま運ぶ)。
- `Account` / `HookNamespace` は**プレースホルダー**。CLI オプション
  (`--account`, `--namespace`)での充填は許すが、ソース属性では書けない。
- **`Flags` はデフォルトでは出力しない(fail-closed)。**
  `--override` 指定時は `hsfOVERRIDE` を**宣言された(非 gap)エントリにのみ**
  付与する。gap の `{"Hook": {}}` に `Flags` を足すと no-op でなくなり
  別 operation として解釈・拒否されるため、**gap オブジェクトは常に厳密に
  空のまま**とする(歯抜け構成 + `--override` の検証ケースを Phase 3 に含める)。
- **`HookParameters` は生成しない**(確定事項 #10)。installed parameter を
  設定したい場合はテンプレートへ手で追加する。wire 上は「omission =
  HookDefinition default の継承」「name のみ = 継承値の消去」「name+value =
  明示設定」の三状態であり、omission は「パラメータ無し」を保証しない。
  この注意はテンプレートのドキュメントに明記する。
- **生成情報は別 sidecar**(`sethook.template.meta.json`)に置き、テンプレート
  本体は protocol-shaped な JSON に保つ。sidecar には RFC 3339 の
  `generated_at`、HookHash、位置情報(`declared` / `gaps` / `untouched_beyond`)、
  `required_amendments` を含める。
- `required_amendments` は **`Hooks` を無条件に含み**、テンプレートの
  フィールドから導出できる範囲(NamedHooks / HookOnV2 / HookCanEmit)を
  加えたものである。**wasm 内の Hook API 使用に由来する amendment 依存は
  カバーしない**(この制限を sidecar のドキュメントに明記する。
  feature→amendment レジストリによる拡張は将来課題)。

### 9.3 テンプレートの意味論: 占有位置パッチ

`{"Hook": {}}` は「この位置に触らない」という**位置合わせの no-op**であり、
「この位置が空である」という表明ではない。したがってこのテンプレートは:

- 歯抜け位置に既にインストール済みの Hook があっても**そのまま残す**
- 最大宣言 index より後ろの位置の既存 Hook も**そのまま残す**
- つまり「チェーン全体の宣言的な実現」ではなく、
  **「宣言した占有位置に対するパッチ(owned-position patch)」**である

このため、**テンプレートの適用が成功しても、アカウント上のチェーン全体の
挙動がソース宣言だけから決まるとは限らない**。運用者は submit 前に対象
アカウントの既存チェーンを確認すること。sidecar の `positions` は
この確認を支援するための情報である。ledger を読んで宣言どおりへ収束させる
(不要位置の削除・置換の導出)は v0.2 の範囲外とする。

同様に、次もテンプレートには**含めない**: 複数アカウント・ネットワーク別の
設定、grant、既存チェーンとの diff、削除操作、パラメータ実値・secret。

テンプレートは、デフォルト(非 `--override`)では「ソース宣言どおりに、
宣言位置が空いているアカウントへ新規インストールする 1 トランザクション」の
編集可能な雛形であり、`--override` 指定時は宣言位置の既存 Hook の置換を
許可する雛形になる。

## 10. 未決事項

| ID | 論点 | 推奨 | 備考 |
|---|---|---|---|
| D1 | struct を跨ぐ state 共有(別クレートのチェーンと同じ state を読む) | v0.2 では対象外 | 型定義クレートの共有(通常の Rust の手段)で対応 |
| D2 | per-hook の state/param 使用宣言(`uses = [...]`) | v0.2 では入れない(全宣言 = 共有スキーマ) | sidecar の宣言は「共有スキーマの転記」とラベルする(§5.4)。検出ベースの絞り込みは将来課題 |
| D3 | weak/collect/again 等の実行モード属性 | v0.2 では入れない | 必要になった時点で属性を追加 |
| D4 | ビルド戦略 A→B の切替条件 | 実測でビルド時間が問題化したら | 同等性の定義は §7 に固定済み(バイト同一性は要求しない) |
| D5 | encoded default の成果物への搬送 | 将来課題 | const 評価可能な encode か、wasm carrier 経由の抽出。実現すればテンプレートへの `HookParameters` 出力を再検討 |
| D6 | 条件付きコンパイル(`#[cfg]`)対応 | v0.2 は禁止(§5.1) | 必要になったら discovery との整合意味論を定義して解禁 |

## 11. 開発者体験への総合評価

| 観点 | 単一 Hook プロジェクト | チェーン(複数 Hook)プロジェクト |
|---|---|---|
| 記述量 | 増(正味 4〜6 行 + 概念 2 つ。§4.1) | 大幅減(クレート統合、宣言共有) |
| 正しさ | ほぼ同等(メタデータ局所化で微改善) | 大幅改善(共有スキーマの型保証、ペア検証、占有位置のコード化) |
| ビルド | 実質不変 | ビルド時間 N+1 倍(leaf のみ)だが手作業の統合が消える |
| デプロイ | テンプレート生成で改善 | 大幅改善(占有位置パッチが 1 JSON で出る) |
| 学習 | struct 儀式 + index の説明が必要 | 「1 struct = 1 チェーン」は概念としてむしろ教えやすい |
| IDE/デバッグ | マクロ複雑化で悪化リスク(span 保持契約・診断カタログ §4.4 で緩和) | 同左 |

総合すると、**この提案の価値はチェーン開発で最大化され、コストは単一 Hook の
儀式増とマクロ実装の複雑化に集中する**。Xahau の実プロダクト(governance、
reward、firewall 群)はチェーン前提のものが多く、ツールチェーンとして
チェーンを一級市民にする判断は妥当である。

## 12. 移行計画

移行元は**現行 v0.0.x API** である(本書で「現行」は常に v0.0.x を指す)。

### 12.1 正規移行表(Phase 1 成果物)

次の対応表を Phase 1 で作成し、機械的な書き換え可能性を検収基準とする:

| 旧(現行 v0.0.x) | 新(v0.2) |
|---|---|
| `metadata! { name, description }` | Cargo package name / `#[hooks(description = ...)]` |
| `metadata! { HookOn / IncomingHookOn / OutgoingHookOn / 全省略 }` | entry 属性 `on` / `on_incoming` + `on_outgoing` / 全省略(+新設 `on = all`) |
| `metadata! { HookCanEmit, HookName }` | entry 属性 `can_emit` / `name` |
| `#[hook] fn` / `#[cbak] fn` | `#[hooks] impl` 内の `#[hook(i, ...)]` / `#[cbak(i)]` |
| `hook_state!`(全宣言形) | `#[state(...)]` フィールド |
| `hook_parameter!` / `otxn_parameter!`(全宣言形) | `#[hook_param(...)]` / `#[otxn_param(...)]` フィールド |
| 各宣言マクロの生成アクセサ | ハンドルのアクセサ(対応表で名称・シグネチャ・意味差を明記。§5.6) |
| 成果物パス(単一 wasm + sidecar) | `target/rshooks/<crate>/current/` 世代構成(§7.5) |

### 12.2 examples / book

- examples 01〜15: 単一 Hook のまま新形式へ機械的に書き換え(移行表の検収対象)。
- **`80_reward` + `81_govern` は 1 クレートのチェーン example へ統合する**
  (この統合こそ本提案の実証であるため、機械的書き換えではなく設計タスクとして
  扱う)。共有 key レイアウト型を struct 宣言に一本化し、index の割り当て
  (genesis アカウントのチェーン構成に合わせる)までを行う。
  **Phase の割り当て**: Phase 1 では「80/81 の全宣言形が新文法で表現可能で
  ある」ことの机上証明まで(移行表の網羅性検収の一部)。統合クレートの実装と
  ビルドは Phase 2(マルチ index が前提)。生成テンプレートによる e2e
  インストール検証は Phase 3。旧 2 クレート版は統合完了時に削除する。
- book はチュートリアル冒頭を新最小形(§8.4)で書き直し、
  「struct は宣言の器で実行時実体はない」「index は占有位置」を最初に説明する。

### 12.3 検収基準

- 全 examples が `-D warnings` でビルドできる(lint 契約 §5.5)
- 移行表のみを参照して旧→新の書き換えが完了できる(追加の口頭知識を要しない)
- e2e: 統合 govern/reward チェーンが生成テンプレートで standalone ノードへ
  インストールでき、既存の e2e シナリオが通る(Phase 3)

## 13. 実装ロードマップ(参考)

1. **Phase 1**: `#[hooks]` struct/impl マクロ(マーカー型注入 §5.4、
   双方向ハンドシェイク §5.1、形状表 §5.1)+ **単一 index で完結する
   Strategy A の安全基盤一式**: BuildPlan(§7.1)、per-index 処理順序
   (§7.2)、discovery/選択ビルドの整合比較(§7.3。CanonicalRecord の
   バイトレイアウト確定は Phase 2 冒頭でよいが、比較そのものは Phase 1 から
   行う)。現行パイプラインは exact な `hook`/`cbak` export を要求するため、
   選択ビルドまで含めて初めて動く成果物になる。正規移行表(§12.1)+
   80/81 表現可能性の机上証明(§12.2)、アクセサ対応表と欠落判定規則
   (§5.6)、診断カタログ + trybuild(§4.4)、compile-fail/pass テスト
   (§5.1)、lint 契約 CI(§5.5)、素の cargo/rustdoc 契約テスト(§7.6)。
   examples 01〜15 と book の移行。
2. **Phase 2**: マルチ index 対応(反復と複数成果物の組み立て)。
   CanonicalRecordV1 バイトレイアウト確定(§7.3)、世代管理(§7.4。
   公開成果物一式は wasm + per-index sidecar)、§6.3 真理値表の実装、
   80/81 統合クレートの実装・ビルド。
3. **Phase 3**: SetHook テンプレート + meta sidecar 生成(§9。世代の公開
   成果物一式にテンプレート類を追加)。歯抜け + `--override` 検証。
   e2e(hooks-toolkit)でテンプレートを使った実インストール検証
   (govern/reward 統合 example を含む)。
4. **Phase 4**(任意): ビルド戦略 B(wasm 分割パス)への差し替え
   (同等性テスト §7 を先に整備)。

## 14. 参照

- [docs/DESIGN.md](./DESIGN.md) — 現行アーキテクチャ(§5.4 エントリポイント、§6 rshooks-build)
- `crates/rshooks-build/src/metadata.rs` — トリガー三形の検証と
  `HookOnIncoming`/`HookOnOutgoing` の wire 直列化(現行実装)
- [Xahau SetHook](https://xahau.network/docs/protocol-reference/transactions/transaction-types/sethook/) — `Hooks` 配列の位置意味論、空 `Hook` オブジェクト、hsfOVERRIDE、Install 時のフィールド継承
- [Xahau HookOn](https://xahau.network/docs/hooks/concepts/hookon-field/) / HookOnV2 / NamedHooks / HookCanEmit amendments
- [Xahau Parameters](https://xahau.network/docs/hooks/concepts/parameters/) — HookParameters の継承・消去・明示設定
