Agent edit tools: implementation research and a client design

Research date: September 7, 2026. Scope: coding agents editing local repositories.

The central finding is that replacement itself is small, but deciding what may be replaced, preserving everything else, and recovering from failure are substantial engineering problems. Public implementations make different choices about all three. A tool reporting success establishes neither that it selected the intended occurrence nor that the resulting program is correct.

My recommendation is a deterministic executor with explicit snapshot identity, unambiguous targeting, literal replacement, preservation of untouched bytes, and truthful commit receipts. Expose the edit syntax each target model handles well. Use fuzzy search to propose recovery candidates; start with exact, grounded edits as the automatic write policy.

This report distinguishes source inspection, documented contracts, historical recovered artifacts, and my design recommendations. Repository heads are pinned below; a repository head can contain development code that is not enabled in a released product. I inspected source and relevant tests, but did not run full agent products or their repository suites. Thirteen named isolated matcher/JavaScript probes were executed across the original five-probe run and two multi-edit follow-up runs; their results appear below. No API key was needed.

**Evidence and revisions.** These revisions identify the implementations discussed, rather than asserting that every deployed version behaves identically.

| Implementation | Inspected revision or evidence | Important qualification |
| --- | --- | --- |
| OpenAI Codex | `5ecb3afd1bf405149e2159bfda50093b0c1b5fab` | Line-ending preservation is feature-gated and disabled by default in this source. |
| Aider | `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` | SEARCH/REPLACE and unified-diff modes use different execution paths. |
| Gemini CLI | `85aca163f6c73ac6ce380b5447359146b8adcae4` | Matching, optional model repair, and scheduling are separate layers. |
| OpenCode | `57ef3828431790c53f8f333c7ffbfe88770a1812` | The inspected dev revision includes both V1 and V2 tools. |
| Pi | `e687434a60174db1a9c961d973881a7a851a0597` | This is Pi, separately from its Oh My Pi fork. |
| VS Code Copilot Chat | `5863f5a7088958050792b5dccbe8b46c6e13eccc` | Findings apply to this public extension implementation. |
| Cline | `c21b17255b228e88a1518c18a73a473ee5876362` | The inspected path is its built-in SDK editor executor. |
| Oh My Pi | `a1b254047d12e143b7c6011536e918c6c35c5906` | Current hashline implementation is in Rust; some documentation still names earlier TypeScript paths. |
| Claude Code | Current official tool reference; changelog at `ab9b2cf7bb9e4f98ff264c07a22e46d83c29c558` | Documentation establishes the public contract, not all backend details. |
| Historical Claude Code MultiEdit | Published `@anthropic-ai/claude-code@1.0.67` package | Actual distributed implementation; current documentation calls MultiEdit legacy. |
| Historical OpenCode MultiEdit | `b5acc2203c1aedd2c5a0e356e47392549d1f88b6`; removed in `2486621ca1b9d35ed15ee6c2ff2a04ba46c8e02a` | Definition was already unregistered immediately before removal. |
| Recovered Claude artifact | `liuup/claude-code-analysis` at `7b7b915d7da804088a8152ed24c68e3da2d1110e` | Historical artifact; claimed package/source-map provenance was not independently authenticated. |
| Anthropic API demo | `claude-quickstarts` at `3313e9716fb5b977248bcd06cb0cc86a8c547b9b` | A sample executor, not the Claude Code backend. |
| Cursor | First-party May 2024 Fast Apply article | Historical architecture; current backend details remain unestablished. |

**There are several distinct editing architectures.** The string supplied by the model might be an exact old/new pair, a patch with context, a line/range reference, a structural operation, or a sketch interpreted by another model. Treating all of these as one API hides important differences.

| Form | What identifies the target | Main advantage | Main failure mode |
| --- | --- | --- | --- |
| Literal old/new | Reproduced source text | Familiar, language-independent, naturally asserts expected content | Transcription mistakes and duplicate text |
| Contextual patch | Old/context lines plus ordered anchors | Compact changes across multiple regions | Incorrect context, parser errors, occurrence selection |
| Snapshot-bound range | A revision plus line/range or opaque span handle | Avoids retranscribing the old block | Stale references, wrong range, identity confusion |
| AST/CST operation | A parsed construct plus revision | Can target functions and structured transformations | Ambiguous symbols, parser coverage, comments and formatting |
| Model-based apply | Current source plus an edit sketch | Handles incomplete or approximate instructions | Another inference can introduce unintended changes |

An API tool schema and its executor are also separate things. OpenAI's Responses apply-patch tool emits operations that the integration must execute. Anthropic's API text-editor tool similarly gives the model an interface whose operations are implemented by the client. Neither API automatically supplies all the filesystem guarantees discussed here. [OpenAI apply-patch guide](https://developers.openai.com/api/docs/guides/tools-apply-patch), [Anthropic text-editor guide](https://platform.claude.com/docs/en/agents-and-tools/tool-use/text-editor-tool).

**Claude Code's current public contract is exact and unique replacement, with a nuanced observation policy.** `Edit` takes `file_path`, `old_string`, `new_string`, and optional `replace_all`. The current reference says no regex or fuzzy matching. Repeated matches require more context or explicit replace-all.

Read-before-edit is model- and permission-dependent. Older models still require a qualifying read. Newer models can edit an unread file when Read is available and would not require permission. A read marked PARTIAL does not qualify. Since v2.1.208, an externally changed file can still be edited when the requested old text matches the current file exactly and unambiguously and reading would not prompt; the result reports that other content changed. This is more permissive than strict whole-file snapshot checking. [Official Edit behavior](https://code.claude.com/docs/en/tools-reference#edit-tool-behavior).

The official changelog is a useful source of regression cases: curly-quote corruption, doubled CRLF, stripping Markdown's trailing-space hard breaks, large-file memory failures, and read-state errors after resuming offset/limit reads all received fixes. This demonstrates why observation tracking and byte handling deserve explicit design. [Pinned changelog](https://github.com/anthropics/claude-code/blob/ab9b2cf7bb9e4f98ff264c07a22e46d83c29c558/CHANGELOG.md).

**The recovered Claude implementation is instructive historical evidence.** The public artifact claims to contain source recovered from an npm source map. Its provenance was not independently authenticated here, and its behavior must not override today's official documentation. [Artifact provenance](https://github.com/liuup/claude-code-analysis/blob/7b7b915d7da804088a8152ed24c68e3da2d1110e/README.md).

Its tool validates path, size, observation state and matching, prepares permissions/backups, rereads before editing, persists, then updates editor/observation state. There is deliberately no asynchronous yield between final reread, freshness check and write. That prevents interleaving in that JavaScript event loop; it does not prevent another process from changing the file. [Historical tool call path](https://github.com/liuup/claude-code-analysis/blob/7b7b915d7da804088a8152ed24c68e3da2d1110e/src/tools/FileEditTool/FileEditTool.ts#L427).

Historical matching first tries literal text, then curly/straight quote equivalence. It counts occurrences of the selected original spelling afterward. That distinction can hide ambiguity: two differently styled quote sequences can normalize to the same search string while the first original spelling occurs only once. A quote-style heuristic can also transform straight quotes in the replacement into curly quotes. Replacement uses a callback so JavaScript dollar substitution tokens stay literal. Deletion can expand its search to consume a following newline. Other normalization trims replacement-line trailing whitespace except for Markdown and translates selected sanitized control-token spellings. [Historical matching, replacement and normalization](https://github.com/liuup/claude-code-analysis/blob/7b7b915d7da804088a8152ed24c68e3da2d1110e/src/tools/FileEditTool/utils.ts#L73).

The historical persistence helper attempts temporary-file write, flush, mode preservation and rename, but falls back to direct writing if that fails. Encoding support and inferred dominant newline style are separate from preservation of every original newline. These are artifact-specific observations, not current production guarantees. [Historical writer](https://github.com/liuup/claude-code-analysis/blob/7b7b915d7da804088a8152ed24c68e3da2d1110e/src/utils/file.ts#L362), [historical encoding/EOL detection](https://github.com/liuup/claude-code-analysis/blob/7b7b915d7da804088a8152ed24c68e3da2d1110e/src/utils/fileRead.ts#L20).

Anthropic's separate API quickstart expands tabs in the entire file, search and replacement before matching. It uses Python's count/replace and direct text writes. This is acceptable evidence of sample behavior, but a poor starting point for a strict byte-preserving repository editor: unrelated tabbed content can change. [Official demo executor](https://github.com/anthropics/claude-quickstarts/blob/3313e9716fb5b977248bcd06cb0cc86a8c547b9b/computer-use-demo/computer_use_demo/tools/edit.py#L167).

**Codex performs ordered, increasingly tolerant line matching.** Its `seek_sequence` scans the eligible window in four complete passes: exact lines, trailing-whitespace-trimmed lines, both-ends-trimmed lines, then a specific Unicode punctuation/space normalization plus trimming. Each pass selects its first hit. A later exact hit therefore wins over an earlier fuzzy hit, but duplicate exact hits are not rejected. Unicode handling covers selected typographic quotes, dashes/minus signs and spaces; it is not general edit distance or AST matching. [Pinned matcher](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/apply-patch/src/seek_sequence.rs#L11-L124).

`@@` is a textual search anchor, not a parsed function or class scope. Matching an anchor advances the cursor past it. Ordinary hunks search the original file from that cursor and advance it after each match. Replacements are then applied without invalidating original offsets. A hunk cannot normally search for text introduced by a previous hunk. A particularly surprising boundary: a hunk with no old lines appends at EOF, even if an `@@` anchor was supplied. Insertion at a specific location needs actual context. [Placement and replacement logic](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/apply-patch/src/file_update.rs#L87-L245).

Line preservation depends on a feature flag. In this revision, `apply_patch_preserve_line_endings` is under development and defaults off. The preservation implementation retains source terminators and source context rather than reconstructing context from model text; new lines use the first observed newline style. It still adds a final terminator to an originally unterminated file. The legacy mode can mix CRLF and LF and can reproduce model-supplied context after tolerant matching. [Feature default](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/features/src/lib.rs#L1148-L1153), [mode selection](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/core/src/tools/handlers/apply_patch.rs#L63-L73), [source-line reconstruction](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/apply-patch/src/text_file.rs#L30-L121).

Preflight and execution are distinct. Execution rereads and reapplies the patch; the inspected path does not use an expected snapshot digest. Writes occur in sequence, with earlier successes retained on later failure. Add can overwrite an existing file; move writes the destination before removing the source. Tests explicitly cover overwrite, partial success, and differing overlap behavior between legacy and preservation modes. These operation names should not be read as create-exclusive or transactional guarantees. [Execution and write semantics](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/apply-patch/src/lib.rs#L487-L724), [regression tests](https://github.com/openai/codex/blob/5ecb3afd1bf405149e2159bfda50093b0c1b5fab/codex-rs/apply-patch/tests/suite/tool.rs#L326-L437).

**Aider has multiple editing engines with materially different policies.** In SEARCH/REPLACE mode, the active order is exact line matching, consistent indentation repair, retry without a spurious leading blank line, then paired ellipsis segments. Exact/indentation matches select the first occurrence. A function named `replace_closest_edit_distance` exists, but its call is below an unconditional return in this path; an 80% fuzzy matcher is not active merely because that function appears in the file.

Indentation repair requires corresponding lines to match after removing leading whitespace and a consistent added prefix across nonblank lines. That prefix is carried into replacement lines. Failed named-file matching can search other files in chat. The dry run does not accumulate earlier edits, while actual writes are sequential. Failed blocks receive useful recovery feedback, and already successful blocks stay written. Truthiness checks also make a successful empty-string result a problematic boundary. [Active SEARCH/REPLACE code](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/coders/editblock_coder.py#L38-L293).

These are concrete design choices: duplicate matches are intentionally covered by first-occurrence tests, and read/write text mode normalizes newline representation rather than preserving mixed endings byte-for-byte. [Duplicate and indentation tests](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/tests/basic/test_editblock.py#L249-L321), [file I/O](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/io.py#L453-L507).

Unified-diff mode normalizes hunks, can split them into smaller changes and progressively reduce context. Its active underlying search/replace has the uniqueness exception commented out and uses Python replacement without a count, replacing all occurrences. A wrapper refuses repeated very short matches, but that is not universal uniqueness. Existing alternative algorithms in the module are not all selected by this caller. [Unified-diff caller](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/coders/udiff_coder.py#L151-L307), [underlying replacement](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/coders/search_replace.py#L434-L445).

**Gemini CLI has four deterministic matching stages and an optional model fixer.** The ladder is exact substring, per-line trimmed equality with indentation rebasing, a whitespace-flexible regex, then weighted Levenshtein matching over fixed-line-count windows. The fuzzy score is `(distance_without_whitespace + 0.1 * (raw_distance - distance_without_whitespace)) / reconstructed_search_block_length`, accepted at at most `0.1`. Only the fuzzy stage is skipped for queries below ten characters or expensive searches; the cost guard is `source_line_count * old_string.length² > 400,000,000`.

The first strategy finding candidates wins, then a separate validator requires one occurrence unless `allow_multiple` is enabled. The regex escapes metacharacters but joins tokens with `\s*`, which does not preserve token boundaries. A traced deduction is that search `return foo` can match the prefix of `returnfoo();`, so replacing it with `return bar` produces `return bar();`. This was checked as a JavaScript regex primitive, not by running Gemini CLI. [Matching, regex and validation](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/tools/edit.ts#L136-L353), [fuzzy scoring](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/tools/edit.ts#L1358-L1495).

On failure Gemini can ask an LLM to repair the search and sometimes replacement, then rematch and validate. The fixer has one attempt, a 40-second timeout and a 50-entry LRU cache; JSON-family files and notebooks are excluded from this repair. A content-hash check refreshes the context before correction if needed. This refresh is not a final conditional write. [Fixer and cache](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/utils/llm-edit-fixer.ts#L15-L199), [refresh and repair gating](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/tools/edit.ts#L544-L805).

Literal replacement protects dollar sequences. Scheduler-level serialization prevents edit tools from running together within that scheduler, while the standard filesystem adapter uses direct `writeFile`. It does not supply a cross-process transaction. Tests cover fuzzy bounds, candidate counts, indentation, newlines and refreshed content. [Literal helper](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/utils/textUtils.ts#L9-L28), [scheduling](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/scheduler/scheduler.ts#L561-L577), [filesystem adapter](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/services/fileSystemService.ts#L33-L40), [matcher tests](https://github.com/google-gemini/gemini-cli/blob/85aca163f6c73ac6ce380b5447359146b8adcae4/packages/core/src/tools/edit.test.ts#L368-L638).

**OpenCode requires a V1/V2 distinction.** The inspected V1 tool tries Simple, LineTrimmed, BlockAnchor, WhitespaceNormalized, IndentationFlexible, EscapeNormalized, TrimmedBoundary, ContextAware and MultiOccurrence replacers, in that order. BlockAnchor uses first/last anchors and middle-line similarity; ContextAware can accept half of the nonempty middle lines matching. A replacer yields an actual source substring; the orchestrator checks uniqueness of that substring, not all approximate candidate positions. Thus a first uniquely spelled indentation variant can be accepted even when several equivalent candidates exist. Fallbacks generally locate the span without rebasing replacement indentation.

V1 single replacement uses slicing, but replace-all passes a replacement string to JavaScript's `replaceAll`, so dollar substitution tokens have special meaning. A canonical-path semaphore covers its read/edit/write/format sequence; formatter output and diagnostics are part of the broader tool result. This inspected path has no final disk-content equality check. [V1 matchers](https://github.com/anomalyco/opencode/blob/57ef3828431790c53f8f333c7ffbfe88770a1812/packages/opencode/src/tool/edit.ts#L219-L736), [V1 execution](https://github.com/anomalyco/opencode/blob/57ef3828431790c53f8f333c7ffbfe88770a1812/packages/opencode/src/tool/edit.ts#L35-L201).

Even V1 tool selection varies by model: its registry selects apply-patch instead of Edit/Write for certain GPT model IDs. This is a useful example of retaining model-specific interfaces rather than forcing every model through one syntax. [Registry selection](https://github.com/anomalyco/opencode/blob/57ef3828431790c53f8f333c7ffbfe88770a1812/packages/opencode/src/tool/registry.ts#L291-L300).

V2 is exact-only and explicitly defers fuzzy correction. It rejects empty search, preserves BOM, converts request EOL to source style, and requires one nonoverlapping match unless replace-all is requested. Both single and all replacement pass strings directly to JavaScript replacement functions, with the same dollar-token issue. V2's mutation service locks the canonical target, rereads bytes and compares with the earlier content before writing. This is a valuable cooperating-writer guard, but a check followed by a write is not an OS compare-and-swap against another process. [V2 edit](https://github.com/anomalyco/opencode/blob/57ef3828431790c53f8f333c7ffbfe88770a1812/packages/core/src/tool/edit.ts#L24-L208), [conditional mutation](https://github.com/anomalyco/opencode/blob/57ef3828431790c53f8f333c7ffbfe88770a1812/packages/core/src/file-mutation.ts#L74-L156).

**Pi combines original-snapshot batches with normalization-based recovery.** Its current schema accepts one file and `edits: [{oldText,newText}, ...]`. It locates every edit in the original file, rejects overlapping regions, applies edits from high to low offsets, and writes once. Matching failures therefore leave the file unchanged; this does not imply crash-atomic writing. The tool also accommodates several legacy argument shapes. [Schema and execution](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/src/core/tools/edit.ts#L21-L212).

Pi's fuzzy equivalence uses NFKC, trailing-whitespace removal, smart-quote/dash mapping and selected Unicode spaces. It tries exact discovery first, but counts duplicates in normalized content even after an exact hit. If any edit needs normalization, the batch shares normalized coordinates. It maps changes to touched lines, reconstructs those from normalized text, and preserves other line text. The wrapper separately normalizes CRLF and bare CR and restores one detected EOL style, so mixed endings can change globally. Unrelated characters on a touched line can still change: an unrelated `ﬁ` ligature can become `fi`. This is wider than preserving every byte outside the requested span. [Normalization, matching and reconstruction](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/src/core/tools/edit-diff.ts#L11-L361).

Pi uses literal-safe concatenation and queues built-in Edit/Write mutations by canonical path, including symlink aliases. Different files can proceed concurrently. The local operations use ordinary filesystem reads/writes. Cancellation during an in-flight write can return an error after bytes changed, another reason for explicit commit-state reporting. Tests exercise batches, untouched-line preservation, aliases and cancellation. [Mutation queue](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/src/core/tools/file-mutation-queue.ts#L1-L61), [batch tests](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/test/tools.test.ts#L334-L433), [queue/cancellation tests](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/test/file-mutation-queue.test.ts#L37-L272).

**VS Code Copilot Chat includes deterministic recovery and optional model repair.** `findAndReplaceOne` tries exact matching, a trimmed-line strategy, a regex allowing trailing spaces and flexible newlines, then mean per-line normalized Levenshtein similarity. Exact/whitespace/regex ambiguity is returned as multiple matches. Similarity instead picks the best score strictly above 0.95; ties keep the first candidate. That fallback is bounded to old strings of at most 1,000 characters/20 lines and files of at most 1,000 lines. Those are computational limits, not correctness guarantees. [Matcher implementation](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/editFileToolUtils.tsx#L238-L515).

On a missing match, a model-gated healing path can call a secondary endpoint to repair the search and sometimes the replacement, then retry application. An edit can therefore be rewritten by another model even though its original interface looks like a string replacement. [Healing integration](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/abstractReplaceStringTool.tsx#L425-L489), [healing implementation](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/editFileHealing.tsx#L66-L203).

Multi-replace prepares edits, detects overlaps, merges successful edits for the same URI and emits editor edits. It can retain successful operations alongside failures. The extension's text-edit stream and the editor applying that stream are separate; this layer should not be described as a filesystem transaction. [Multi-replace](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/multiReplaceStringTool.tsx#L90-L193).

**Cline's inspected SDK executor is comparatively small.** It reads the file, detects EOL style, converts supplied old/new strings to that style, counts nonoverlapping exact occurrences, rejects zero or multiple, and replaces using a callback to preserve literal dollar tokens. It then writes directly and produces a compact diff. Insertion is a separate one-based line-boundary operation; absent files are created. This path should not be confused with older articles about Cline's SEARCH/REPLACE block parser, or assumed to describe every host-level safeguard. [SDK editor](https://github.com/cline/cline/blob/c21b17255b228e88a1518c18a73a473ee5876362/sdk/packages/core/src/extensions/tools/executors/editor.ts#L162-L263), [EOL policy](https://github.com/cline/cline/blob/c21b17255b228e88a1518c18a73a473ee5876362/sdk/packages/core/src/extensions/tools/executors/line-endings.ts).

**Cursor's published Fast Apply architecture uses another inference.** Its May 14, 2024 article describes a planner producing an edit sketch and a separately trained apply model rewriting the full file. A fine-tuned Llama-3-70B model and speculative edits made that generation fast. The reported evaluation involved about 450 edits on files under 400 lines with model grading. That evidence explains the architecture; it does not establish the best approach for today's models or larger client repositories. [Cursor's historical engineering article](https://cursor.com/blog/instant-apply).

Speculation exploits the fact that much of the output is predictable from the original file. It reduces inference latency; it does not establish correct intent, preserve external edits, or provide commit isolation. A captured historical tool schema also exposes `edit_file` sketches and a stronger-model `reapply`, but captured prompts are not backend source. Current routing, matching and persistence details remain uncertain. [Fireworks' account](https://fireworks.ai/blog/cursor), [captured historical interface](https://github.com/x1xhlol/system-prompts-and-models-of-ai-tools/blob/1e4203a7d88873c1b37ab2d1c07074fea498c274/Cursor%20Prompts/Agent%20Tools%20v1.0.json).

**Hashline shifts the transcription burden into references and revision checks.** The original proposal annotates reads with line numbers and short hashes, then lets the model replace ranges or insert around those references. This avoids emitting the old block again. Read and search output are part of the protocol: edit references must come from the same representation. [Original proposal and benchmark](https://stencil.so/blog/the-harness-problem).

The current Oh My Pi revision has evolved into whole-file tags plus numbered operations, including optional syntax-tree block targeting. Its Rust store derives a four-hex tag from an xxHash value after trimming line-ending spaces/tabs/CR; snapshots also retain text and seen ranges. The patcher has a matching-live-tag path and stale recovery. A four-hex tag is only 16 bits, and this normalization deliberately equates some byte-distinct files. It should not be copied as a strong raw-byte revision check. [Tag and snapshot implementation](https://github.com/can1357/oh-my-pi/blob/a1b254047d12e143b7c6011536e918c6c35c5906/crates/pi-edit/src/store.rs#L69-L83), [tag validation and recovery dispatch](https://github.com/can1357/oh-my-pi/blob/a1b254047d12e143b7c6011536e918c6c35c5906/crates/pi-edit/src/modes/hashline/patcher.rs#L175-L250).

Recovery maps old lines onto current lines and checks neighboring context, with extra treatment for duplicate lines. This is substantially more machinery than checking a tiny hash. Documentation describes preparation before writes and possible partial application after OS write failures. Its exact protocol is version-specific. [Recovery implementation](https://github.com/can1357/oh-my-pi/blob/a1b254047d12e143b7c6011536e918c6c35c5906/crates/pi-edit/src/modes/hashline/recovery.rs#L68-L189), [current edit contract](https://github.com/can1357/oh-my-pi/blob/a1b254047d12e143b7c6011536e918c6c35c5906/docs/tools/edit.md).

**The original five isolated probes made several risks concrete.** I extracted only inspected pure string helpers from the pinned Copilot file, stripped TypeScript types with Node's built-in facility, and ran these alongside ordinary JavaScript demonstrations. This did not execute the extension, its dependencies, or any product backend.

| Probe | Actual result |
| --- | --- |
| Similarity match requested `const permitted = request.role === 'admin';` against actual `const permitted = request.role !== 'admin';` | Accepted; similarity `0.9767441860465116`. |
| Two identical actual candidates in the preceding case | Returned similarity success at offset `0`; first tie selected. |
| Exact helper searching `aa` in `aaa` | Reported one nonoverlapping match and produced `Xa`. |
| JavaScript replacement string `$& $$` | Direct replace expanded it to `TOKEN $`; callback inserted literal `$& $$`. |
| Collapsing whitespace / NFKC | Distinct string-literal spacing became equal; `①` normalized to `1`. |

The run printed `5 probes passed` and exited 0 under Node `v24.10.0`. Node also emitted its experimental-feature warning for `stripTypeScriptTypes`. These observations establish helper behavior, not the frequency of production failures or full-product exploitability.

**Multi-edit tools introduce a second correctness problem: how individually valid replacements interact.** The follow-up inspected ordering, overlap handling, and partial failure, including cases where the tool description promises more than its executor delivers.

“Multi-edit” can mean several distinct things:

- **Replace-all:** one old/new pair applied to multiple occurrences.
- **Single-file batch:** several different replacements in one file.
- **Multi-file batch:** replacements across several files.
- **Parallel Edit calls:** separately executed operations, with whatever coordination the surrounding agent provides.

These interfaces can have different guarantees. The first decision is what version of the text each edit searches. Suppose the file contains `A`, and the request contains:

```text
1. A → B
2. B → C
```

Under **sequential semantics**, the second edit searches the first edit's output, producing `C`. Under **original-snapshot semantics**, both search the original file. The second edit cannot find `B`, so an all-or-none batch rejects the request. Neither interpretation follows automatically from an `edits[]` array.

| Implementation | Batch execution | Failure behavior |
| --- | --- | --- |
| Historical Claude Code `MultiEdit`, v1.0.67 | One file; sequential changes staged in memory, with restrictions on dependencies | Calculation must finish before the planned content is written. [Published implementation](https://unpkg.com/@anthropic-ai/claude-code@1.0.67/cli.js) |
| Pi `edit` | One file; every replacement targets original content; rejects overlapping spans | Missing, ambiguous, or overlapping targets reject the plan before writing. [Implementation](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/src/core/tools/edit-diff.ts#L300-L361) |
| Copilot `multi_replace_string_in_file` | Independently prepares replacements, checks conflicts, then combines successful edits by file | Can retain successful operations while reporting failed ones. [Preparation](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/abstractReplaceStringTool.tsx#L115-L154), [combination](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/multiReplaceStringTool.tsx#L128-L161) |
| Removed OpenCode `MultiEdit` implementation | Loops over ordinary Edit executions, each of which writes separately | Earlier writes can survive a later failure. This definition was already unregistered immediately before removal. [Executor](https://github.com/anomalyco/opencode/blob/b5acc2203c1aedd2c5a0e356e47392549d1f88b6/packages/opencode/src/tool/multiedit.ts#L37-L56), [registry](https://github.com/anomalyco/opencode/blob/b5acc2203c1aedd2c5a0e356e47392549d1f88b6/packages/opencode/src/tool/registry.ts#L179-L219) |

These are version-specific findings, including historical implementations.

**Historical Claude validates against one state and executes against another.** The published v1.0.67 tool validates each replacement against the original disk file, then executes against evolving in-memory content. It rejects some explicit dependencies, including a later search contained in an earlier replacement. It does not recheck uniqueness after every intermediate change.

An extracted-helper probe produced:

```text
Original:
axb
ab

Edit 1: replace "x" with ""
Intermediate:
ab
ab

Edit 2: replace "ab" with "Z"
Result:
Z
ab
```

Originally, `ab` uniquely identified the second line. Deletion created another occurrence, and the later replacement selected that newly created first occurrence. The original uniqueness check no longer protected the target. This was reproduced from published v1.0.67 helpers, not a current-product test. [Historical implementation](https://unpkg.com/@anthropic-ai/claude-code@1.0.67/cli.js)

Current Claude documentation explicitly calls MultiEdit **legacy**. Its exact removal version was not established. [Official documentation](https://code.claude.com/docs/en/permissions#read-and-edit)

**Copilot demonstrates why the description alone is insufficient.** Its description says replacements apply “sequentially,” but preparation uses `Promise.all`; earlier replacement output is not passed into later preparation. Conflicting later edits are marked unsuccessful. An existing regression test explicitly expects both a successful file edit and a conflict error for the second replacement. This is independent preparation before application, not a guarantee that every preparation observes one shared immutable snapshot. [Description](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/multiReplaceStringTool.tsx#L22), [regression test](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/extension/tools/node/test/multiReplaceStringTool.spec.tsx#L157-L245)

**OpenCode's historical description overstated atomicity.** It promised all-or-none application, but its child Edit wrote and formatted the file before the next child ran. The wrapper did not add rollback or a batch-level lock. Formatting could therefore affect the next search. The unused tool was removed on April 21, 2026 and is absent from the current pinned revision. This is a historical code example, not a claim about an active deployed tool. [Description](https://github.com/anomalyco/opencode/blob/b5acc2203c1aedd2c5a0e356e47392549d1f88b6/packages/opencode/src/tool/multiedit.txt#L15-L24), [write/format sequence](https://github.com/anomalyco/opencode/blob/b5acc2203c1aedd2c5a0e356e47392549d1f88b6/packages/opencode/src/tool/edit.ts#L108-L154), [removal](https://github.com/anomalyco/opencode/commit/2486621ca1b9d35ed15ee6c2ff2a04ba46c8e02a)

**Even disjoint edits can interact through normalization.** Pi switches the batch into normalized matching space when any member needs fuzzy recovery. A probe reproduced this consequence:

```text
An exact edit alone:
const x = 2; // ﬃ

The same edit batched with a fuzzy edit elsewhere:
const x = 2; // ffi
```

The comment's ligature changed despite being outside that replacement's requested text. The fuzzy member affected reconstruction of another edited line. The probe's other replacement searched for ASCII `"hello"` against typographic quotes elsewhere in the file. [Normalization and reconstruction](https://github.com/earendil-works/pi/blob/e687434a60174db1a9c961d973881a7a851a0597/packages/coding-agent/src/core/tools/edit-diff.ts#L300-L361)

A useful batch property follows: **adding an independent edit should not change how an existing edit is applied.**

**Eight additional isolated probes verified the multi-edit findings.** Five probes exercised extracted Pi helpers; three exercised historical Claude helpers and validation methods with a synthetic read-only filesystem and a stubbed diff renderer. The products and their repository suites were not run.

| Follow-up probe | Actual result |
| --- | --- |
| Pi: disjoint edits in either array order | Same expected output in both orders. |
| Pi: `A→B`, then `B→C`, with original `A` | Rejected because `edits[1]` could not be found. |
| Pi: overlapping `abc` and `cde` in `abcde` | Rejected the complete helper plan. |
| Pi: valid first edit and missing later target | Rejected the complete helper plan. |
| Pi: exact edit alone versus batched with fuzzy recovery elsewhere | Preserved `ﬃ` alone; changed it to `ffi` in the batch. |
| Historical Claude: original `a\nb`, edits `a→b`, `b→c` | Both original-file validations succeeded; the batch helper rejected the dependency. |
| Historical Claude: deletion creates another later target | Both validations succeeded; the newly created first occurrence was replaced, producing `Z\nab`. |
| Historical Claude: overlapping original targets | Both original-file validations succeeded; later in-memory matching failed and the batch helper threw. |

Both follow-up runs exited **0** under Node **v24.10.0**. The Pi run used `node --input-type=module` with an in-memory extraction and Node's `stripTypeScriptTypes`, which emitted its experimental-feature warning. Their final output lines were:

```text
5 isolated Pi helper probes passed.
3 bounded extracted-helper probes passed; no product execution or filesystem writes.
```

These eight follow-up probes are additional to the original five above. They establish helper behavior, not production failure rates or full-product filesystem guarantees.

**The engineering depth comes from the following distinctions.** These are design conclusions, not claims that every audited agent handles them.

- Match success versus target correctness: an exact match can be in the wrong function. A fuzzy score is a string metric, not the probability of correct intent. A unique match after a concurrent edit can be a different surviving occurrence.
- Candidate finding versus ambiguity checking: use the same equivalence relation for both. If discovery trims lines or normalizes quotes, checking only the selected original spelling is insufficient. Overlapping occurrences need an explicit policy too.
- Context versus replacement: context used to locate a patch should not be rewritten from the model's approximate copy. Preserve it from the original snapshot. Track changed spans separately.
- Observation versus revision: knowing a file was read once is weaker than knowing which bytes and ranges were shown. Timestamp equality is weaker than content equality. A file hash without a path binding is weaker than a snapshot identity.
- Display versus storage: line-number prefixes, truncation markers, CRLF normalization, Unicode code points, JavaScript UTF-16 indices, and UTF-8 byte offsets are different representations. Confusing them moves boundaries or changes content.
- Serialization versus conflict protection: an in-process queue or synchronous critical section excludes cooperating calls in that runtime. It does not exclude editors, formatters, Git operations, or another agent process.
- Atomic visibility versus durability versus transactions: replacing one file atomically, surviving a crash durably, preventing lost updates, and committing several files together are different guarantees.
- Applied versus validated: a file can be committed successfully and then fail syntax or tests. Reporting that as a generic edit failure encourages the agent to apply the same change twice.

**The client design I would build begins with a shared deterministic core.** The recommendation below assumes local repository files, potentially multiple agent tasks, and an emphasis on correctness over maximizing the raw acceptance rate.

Make batching a capability of the core editor: a single Edit is a batch containing one replacement. Keep representation adapters thin. Start with old/new replacement for general models; support a contextual patch adapter for models trained on that interface. Both should produce the same internal plan of explicit source spans and replacement bytes. A line/range adapter can be evaluated without duplicating write logic. Accepting V4A syntax does not require inheriting Codex's first-match policy; document any stricter behavior in the tool description.

The read operation should issue an opaque handle, for example `s17`, associated with workspace, canonical target, raw-byte digest, encoding and display mapping, and exposed ranges. The model copies a short handle; the executor retains a strong digest or the complete immutable snapshot. Do not require the model to calculate hashes, and do not use a truncated display hash as the sole correctness check. Decide whether filesystem identity changes with identical bytes matter for the product.

A minimal single-file contract could look like this; it is a proposed API, not an existing vendor interface:

```json
{
  "snapshot": "s17",
  "operation_id": "edit-42",
  "changes": [
    {
      "old_text": "const retries = 2;",
      "new_text": "const retries = 3;"
    }
  ]
}
```

Default to exactly one eligible match per change. For deliberate bulk replacement, require explicit all-occurrences mode and preferably an expected count. Use distinct create, delete, replace and insert operations; do not overload empty search to mean several of them. A successful result containing an empty file is still success. Treat no-op as a distinct outcome, not as missing match.

Under strict snapshot checking, a validated range handle can identify the target without retranscribing old text. Old text remains useful as an optional assertion and a compatibility format. The important coupling is revision plus target, not any particular spelling of the request.

**Define the batch contract before implementing it.** I recommend snapshot-relative operations by default: all matches are resolved against the same immutable base, overlaps are rejected, and replacements are spliced without changing subsequent coordinates. Use half-open internal byte ranges. Specify zero-width insertions, multiple inserts at the same offset, replacement-boundary insertions, duplicate operations and stable order.

A sequential adapter must simulate its complete sequence in memory before producing the final plan. For example, `a→b` followed by `b→c` has different semantics from two independent changes against the original file. A deterministic executor still produces the wrong result if its adapter silently changes that meaning. In a sequential adapter, matching and uniqueness checks must use the same intermediate state as execution; validating only against the original file is insufficient.

For the default snapshot-relative file batch, validate the complete plan before mutation, require unambiguous targets, and reject overlapping replacement spans. Initially, have the model combine overlapping old-text blocks into one replacement. Sort resolved ranges and assemble output from original spans, preserving bytes outside them. Hold coordination across the whole file operation and revalidate its revision before committing once. All replacements being validated remains a separate guarantee from atomic filesystem replacement.

For multi-file batches, report which files committed, failed, or have uncertain outcomes; a generic failure must not invite replay of already applied edits. Run formatting and diagnostics at a defined boundary, preferably once after each file's complete batch, and record formatter changes in its resulting revision.

Batching can reduce tool round trips, repeated reads, formatting, and diagnostics. It does not automatically reduce matching work: a wrapper may still scan and reconstruct the whole file for every replacement. Resolve targets first, then build output once; optimize candidate search further only when measurements justify it.

**Use a conservative automatic matching policy and rich failure output.** Start with exact matching against the tool's defined view, including an EOL mapping if reads display LF for CRLF. Preserve an explicit map back to original byte spans. Never normalize the whole stored file merely to make a match work.

If no exact match exists, diagnostic search may try consistent indentation shifts, trailing whitespace, Unicode differences, or token-aware similarity. Return a bounded candidate list with exact current text, snapshot/range handles and a concise description of the difference. The agent can then issue an exact edit. Two plausible candidates should result in ambiguity, even if one barely outscores the other. A stricter policy is particularly valuable for operators, identifiers, literals, Python/YAML indentation, Makefile tabs and Markdown trailing spaces.

If later measurements justify automatic repair, add one narrowly defined transformation at a time. Record the transformation and candidate count, and evaluate wrong-target acceptance independently. A threshold such as 0.95 should never be presented as 95% correctness confidence.

**Reject stale revisions by default.** An unchanged old string elsewhere in the live file does not prove the intended target survived. Initially, return a fresh observation and require replanning. An optional rebase can later use a base-to-current change map and guard context to prove an unambiguous textual mapping. Insertions need surrounding guards because their old span is empty. Return a new base and rebased plan; do not silently mutate the request's original revision.

Even a clean textual rebase does not prove that changing surrounding code left the edit semantically appropriate. This is a product policy decision. Strict file revisions cost retries after unrelated changes, while looser range-based revalidation permits more concurrency. Measure both against actual client workflows.

**Preserve bytes outside declared edits.** Track original EOL sequences and BOM/encoding. Preserve mixed endings and an absent final newline unless a deliberate operation changes them. Give newly inserted lines a defined local or repository EOL policy. Reject unsupported/invalid encodings clearly instead of replacing undecodable bytes. Do not silently convert tabs, punctuation, nonbreaking spaces or Unicode normalization forms.

For patch syntax, retain original context lines verbatim. For editor buffers, use the document's version and edit API as the authority; direct filesystem writes can otherwise overwrite unsaved buffer changes. Make formatting a separately logged transformation or a clearly declared composite operation. Its output must update the same snapshot and undo chain.

**Commit safety should have an explicit ceiling.** Serialize cooperating mutations by canonical target and revalidate the snapshot immediately before persistence. Use a platform-appropriate same-directory atomic replacement where supported, preserving the metadata the product promises. Do not silently fall back to truncate-and-write while still claiming atomicity.

Canonical paths alone do not solve hard-link aliases or mutable symlink/parent paths. Define path containment, symlink-following, destination-overwrite and file-identity policies. Check the actual target again at commit. Atomic replacement may change hard-link behavior, modes, ACLs or other metadata; preserve or explicitly constrain those cases.

A recheck followed by rename is not a portable compare-and-swap against arbitrary external writers. Full exclusion requires cooperating writers or a platform/host mechanism that supplies that guarantee. State this limitation rather than calling the operation race-free. Crash durability also requires a separate persistence contract. Platform details matter: Node documents that cancelling a write can still leave written data, while Windows ReplaceFile has explicit metadata/access-right handling. [Node filesystem contract](https://nodejs.org/api/fs.html#fspromiseswritefilefile-data-options), [Windows ReplaceFile](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).

For multi-file requests, prepare all plans before writing. Either build a real transactional storage/host mechanism, or report the exact commit boundary: which files committed, which did not, and whether any outcome is unknown. A journal supports recovery; it does not automatically make multiple filesystem renames atomic. Avoid automatic rollback that could erase intervening user work.

**Make receipts and retries part of correctness.** Bind an operation ID to immutable arguments and store its outcome. A retry should return the original receipt, even if later edits changed the file. Reuse of an ID with different arguments must fail. Cover the crash window between filesystem replacement and recording the receipt; distinguish prepared, committed, rejected-with-no-change, partially committed and outcome-unknown states.

Return actual post-write state, changed ranges, before/after references, match strategy, and a concise diff or diff reference. Keep commit, formatting and validation statuses separate. If tests fail after the write, the response must still say the edit committed. Conditional undo should require the expected after-state, including any formatter output, and must not restore a backup over newer work.

**A small, layered evaluation will tell you more than acceptance rate.** There are two independent evaluation tasks: whether the executor faithfully applies a fully specified edit, and whether a model using its interface completes a coding task. Keep them separate.

For the deterministic executor, test expected bytes, eligible occurrence sets and filesystem effects. A useful corpus includes:

| Category | Cases |
| --- | --- |
| Basic boundaries | Empty file; delete-to-empty; empty search rejected; no-op; BOF/EOF insertion; no final newline |
| Ambiguity | Duplicate blocks; overlapping `aa` in `aaa`; exact later versus fuzzy earlier; two equal fuzzy candidates; normalized candidates with different original spellings |
| Language-sensitive text | `==` versus `!=`; identifiers differing by one character; significant indentation; Makefile tabs; spaces inside literals; Markdown hard breaks |
| Representation | LF/CRLF/bare CR/mixed endings; BOM; supported UTF-16; emoji before the target; combining characters; invalid encoding; literal dollar and backslash sequences |
| Batches | Disjoint edits and order independence; overlapping context/hunks; sequential dependencies; newly created matches; insertion-induced offset shifts; same-offset inserts; one malformed or missing-target operation; mixed fuzzy/exact members; formatter changes between operations; same file through two aliases; retry after partial success |
| Concurrency | Change before validation; change between validation and commit; editor buffer differs from disk; formatter writes; retry after a lost response |
| Persistence | Read-only target; full disk; interrupted write; create collision; move collision; failed later file; conditional undo after subsequent edits |
| Scale | Huge/minified line; repeated near-matches; large replacements; bounded diagnostic search and output truncation |

Properties are especially useful: untouched bytes remain identical; rejection means no mutation; independent nonoverlapping changes commute when the API says they should; adding an independent edit does not change another edit's matching or reconstruction; replayed operation IDs do not duplicate changes; and conditional undo restores exactly the original bytes when its precondition holds. Use fault injection for I/O failures and mutation testing for match/cardinality and commit-state logic when configured. No new production implementation or test suite was built in this research task.

For the model-facing experiment, hold model version, prompts, retrieval, tool budgets, diagnostics and retry policy constant while changing edit representation. Then run a separate tuned-interface comparison, because an interface with unfamiliar syntax may need a different description. Compare exact replace, bounded-recovery replace, contextual patch and snapshot-range references on the client's languages and file sizes.

Measure correct target and correct final change, unintended changed bytes, false acceptance, false rejection, first-attempt application, successful repair within budget, tokens including rereads/diagnostics, and end-to-end latency. Stratify failures by parser, target resolution, conflict, I/O, and program semantics. Inspect both failed calls and suspiciously successful calls. Test-pass rate alone can miss an edit to untested code.

**Existing benchmarks support experimentation, not a universal winner.** Diff-XYZ studies apply, reverse-apply and diff generation on real-commit triples. Search/replace works well for stronger models in its experiments, but the paper explicitly excludes full tool-use/reasoning loops and warns against treating these results as direct production predictions. [Diff-XYZ paper](https://arxiv.org/html/2510.12487v2).

The hashline author's experiment used 180 mutation tasks per run, three runs and 16 models, finding large format-dependent differences. Its current page also includes revised results, so old headline numbers should be tied to their experimental revision. A separate small multi-language experiment found mixed results and no fuzzy-fallback use in its observed edits. These experiments support testing the model/interface combination; they do not establish that more fuzziness or hashline always wins. [Original hashline experiment](https://stencil.so/blog/the-harness-problem), [independent experiment and linked data](https://nwyin.com/blogs/hashline-vs-replace-edit-bench).

An AST-targeted experiment reported strong results across 29 Python tasks and four models. Its language and task scope are useful but narrow. EDIT-Bench contributes 540 real-world instructed editing problems with richer cursor/context information; it evaluates editing ability rather than isolating a filesystem matcher. [AST experiment](https://geometricagi.github.io/2026/04/02/ast-edits.html), [EDIT-Bench](https://arxiv.org/abs/2511.04486).

For the first client version, I would implement exact replacement, explicit single-file batches, snapshot receipts, careful byte handling and actionable mismatch errors. I would evaluate a range-reference adapter alongside it. Automatic fuzzy commits, a dedicated apply model, semantic refactoring, and stale-snapshot rebasing should be added when measured failures justify their complexity. AST/LSP operations are especially appropriate for real symbol refactoring, where changing every matching string cannot establish semantic scope.

The research does not establish current proprietary Cursor internals or every deployment of Claude Code, and the isolated probes are not an end-to-end product benchmark. Those limits do not prevent building and evaluating the deterministic core above.
