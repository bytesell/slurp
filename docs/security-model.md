# Slurp security model

What slurp defends against, how, and where it deliberately does not. The reporting policy is in [SECURITY.md](../SECURITY.md).

Every property here names the file and the symbol that enforces it, so a claim can be checked. If you find one that the code does not deliver, that is a bug in its own right.

## Threat Model

### Users and Trust Levels

**Theme developers** write `.slurp` files and run `slurp build`. They are trusted to write well-intentioned templates but are not trusted with secrets outside explicitly designated middleware files. They control the full template surface.

**End users** view the rendered HTML in a browser. They are untrusted: their data may be stored by the application and reflected back into templates via context variables. They must not be able to inject executable HTML through any expression path.

**Operators** deploy the compiled output and run the image optimizer and dev server. They control environment variables, network configuration, and reverse proxy setup.

### Attack Surfaces

| Surface | Attack Type | Mitigated By |
|---|---|---|
| `${ expr }` in HTML text | XSS via reflected user data | HTML entity escaping (`renderer::escape::escape_html`), always on |
| `${ expr }` in a plain attribute value | Attribute breakout | Attribute escaping (`renderer::escape::escape_attr`), always on |
| `${ expr }` in a URL attribute | `javascript:` / `data:` navigation | Scheme check with whitespace, zero-width and bidi normalisation (`renderer::has_forbidden_url_scheme`, routed by `renderer::is_url_attr`) |
| `${ expr }` in a `<script>` body | XSS | **Compile error** unless the value carries `\| js` or `\| json` (`lib.rs` `check_node`, error `UnsafeScriptInterpolation`), with a render-time backstop in `Renderer::render_script_expression` |
| `${ expr }` in a JS-evaluated attribute, inside a JS string literal | JS string breakout | JS-literal escaping (`renderer::escape::escape_js_string`), applied automatically; context resolved by `renderer::js_slot::slot_quote_contexts` |
| `${ expr }` in a JS-evaluated attribute, in statement position | Statement injection | Emitted as a self-delimiting JSON literal (`renderer::json_literal`); a false `\| js` claim there is a compile error |
| `${ expr }` in a `style` / `:style` attribute | CSS declaration injection, `url()` beacons | Structural characters removed (`renderer::escape::escape_css_value`), routed by `renderer::is_css_attr` |
| `${ expr }` in `srcdoc` | HTML injection into a nested document | Ammonia allowlist sanitizer run on the value (`Renderer::srcdoc_guard`) |
| `{html expr}` raw output | XSS from developer-supplied HTML | Trust assumption; `sanitize` flag available |
| `{html expr sanitize}` | XSS via sanitized HTML | Ammonia allowlist sanitizer |
| `{html expr}` as an attribute spread | Handler and scheme injection | Whole-attribute re-parse, unsafe names and schemes dropped (`renderer::sanitize_attr_spread`, name gate `renderer::is_safe_attr_name`). **Theme-trust only** - see below |
| `env.SLURP_SECRET_*` access | Secret exfiltration | Literal name-prefix check on the dotted AND subscript forms, refused in every file including middleware (`lib.rs` `check_env_key_name`) |
| Middleware files | Data exfiltration, auth bypass | Compile-time root allowlist: `request`, `env`, `loop` only (`lib.rs` `check_path`) |
| Image optimizer `src` parameter | SSRF, path traversal, decompression bombs | IP routability allowlist, connection pinned to the validated addresses, scheme allowlist, magic bytes plus a decoder-format allowlist, size and pixel caps |
| Image optimizer request handling | DoS by CPU, memory or connection exhaustion | Transform work on the blocking pool behind a global concurrency permit, whole-request timeout, per-client rate limit with a bounded bucket table |
| Dev server static file serving | Path traversal | Request-path rejection of `..` plus a canonicalized containment check |
| Dev server WebSocket (HMR) | Cross-site WebSocket hijacking | `Origin` allowlist (loopback + this server's port), loopback peer check as defence in depth |
| Dev server HTTP surface | DNS rebinding | `Host` header allowlist (loopback names only, `--allow-host` to opt in) |
| Dev server backend proxy | Cross-site confused deputy | `Origin` allowlist; upstream CORS headers are not relayed |
| Runtime `{fetch}` blocks | Editor mock data reaching production | `window.SLURP_FETCH_INTERCEPTOR` ignored in production mode (`runtime/src/fetch/index.ts`) |
| Import resolution | Circular imports causing hangs | Cycle detection at parse time (`lib.rs` `detect_circular_imports`), chain length capped by `MAX_IMPORT_CHAIN` |
| `{each}` / `{repeat}` iteration | DoS by loop count | `MAX_ITERATIONS` (1,000) per construct plus the global `MAX_TOTAL_ITERATIONS` (1,000,000) per render |
| Filter chains | DoS by memory amplification | `MAX_FILTER_VALUE_BYTES` (8 MiB) per intermediate and `MAX_FILTER_TOTAL_BYTES` (64 MiB) per render |
| Any render | DoS by output size or recursion | `MAX_OUTPUT_BYTES` (16 MiB), `MAX_RENDER_DEPTH` (96) |

---

## Security Properties

### Escaping (Compiler)

In HTML text context, all `${ expr }` expression output is HTML-escaped before being written into the rendered document. The escaping is applied by the renderer's `Node::Expression` arm in `Renderer::render_node`, calling `escape_html` in `renderer/escape.rs`, and covers the five characters that are dangerous in HTML text context:

| Character | Escaped Form |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&#39;` |

`/` is deliberately not escaped: it has no special meaning in HTML text, and escaping it would corrupt URLs and date strings.

This escaping is **compile-time and always on**, and there is no opt-out.

**HTML entity escaping alone is not the right answer everywhere, and slurp does not apply it everywhere.** The renderer picks an escaper from the context the slot lands in: a `<script>` body, a JavaScript-evaluated attribute, a `style` attribute and `srcdoc` each get their own treatment. The next section is the whole of that behaviour, including the places where the answer is a refusal rather than an escaper.

#### How `${ }` is treated in each context

Read this before interpolating untrusted data anywhere other than HTML text or an ordinary attribute.

**1. Inside a `<script>` body: a compile error unless you declare the context.**

`<script>` is a raw-text element: the HTML parser does not decode character references inside it. HTML entity escaping is therefore the wrong escaper and stops nothing, and no automatic escaper is correct either, because `escape_js_string` is built for a value sitting *inside* a JS string literal and would leave `1;alert(1)` completely intact in `var n = ${ count }`.

So slurp refuses instead. `Renderer::render_element` tracks `script_depth`, `Renderer::render_node` routes an expression under it to `render_script_expression`, and an expression whose filter chain contains neither `js` nor `json` (`renderer::expr_has_script_filter`) is rejected:

```slurp
<script>var n = ${ count };</script>
```

```
error[UnsafeScriptInterpolation]: an expression in a <script> body must end with
the `| js` filter (data inside a JS string literal) or `| json` (a bare JSON value) ...
```

The rule is enforced twice, on purpose. `lib.rs`'s security walk carries an `in_script` flag on `CheckCtx` and raises the error at compile time, which is what `slurp build` and `slurp validate` report. `Renderer::render_script_expression` is the render-time backstop for the paths that render without going through the security gate (section rendering, and the WASM `render` exports); there it emits nothing and records a diagnostic rather than falling back to an escaper that does not fit.

**A filtered value is emitted verbatim into a script body, and that exemption is what makes `| js` usable.** Before it, `var a = "${ v | js }"` rendered a literal `&#39;` inside the JS string, so there was no working way to interpolate into a script at all. The exemption is reached only while `script_depth > 0`; everywhere else `| js` output is still entity-escaped.

```slurp
<script>var a = "${ v | js }";</script>   {* v = ');alert(1);('  ->  var a = "\');alert(1);(\'"; *}
<script>var b = ${ v | json };</script>   {* renders "');alert(1);('" - a self-delimiting JSON string *}
```

`{$let}` remains the option that needs no filter: it serialises to a JSON `<script type="application/json">` payload through `escape_script_safe`, which encodes `<` as `<` (killing both `</script>` breakout and `<!--` comment-state confusion) and the two JS line separators, while staying valid JSON.

**2. Inside a JavaScript-evaluated attribute: escaped automatically, by position.**

This covers native `on*` handlers and every Alpine attribute that evaluates its value as JavaScript: `@click`, `x-on:*`, `x-data`, `x-init`, `x-effect`, `x-show`, `x-text`, `x-html`, `x-if`, `x-for`, `x-model`, and the `:` / `x-bind:` binding forms. The set is `renderer::is_js_attr`, which strips modifier suffixes so `x-model.lazy` is recognised.

Entity escaping alone is not enough here, and `escape_attr`'s own doc comment says so: the HTML parser decodes character references in an attribute value **before** Alpine or the event-handler compiler reads the string. So `Renderer::eval_interpolated` escapes each `${ }` slot *before* the attribute escaper runs, per slot rather than over the whole expanded value, which keeps the theme's own quotes intact while neutralising only the data between them.

Which escaper is right depends on where the slot lands, and that is a lexical property of the author's own text. `renderer::js_slot::slot_quote_contexts` walks the literal segments between the slots and reports, for each slot, the quote character of the JS string literal enclosing it or `None`.

*Inside a string literal* - `escape_js_string` is a complete answer, and is applied:

```slurp
<button @click="add('${ v }')">Add</button>
```

With `v` set to `');alert(1);('` this renders `add('\');alert(1);(\'')`. The value stays one JS string literal. Entity escaping in an attribute is reversible, so composing `escape_attr` on top afterwards is harmless: the browser decodes the entities and the backslashes are still there.

*In statement or expression position* - `escape_js_string` is no answer at all, because `;` and `(` have no JS string escape. `renderer::json_literal` encodes the value as a JSON literal instead, which **is** self-delimiting whatever it contains: a string becomes a quoted string, a number stays a bare number, and neither can contribute a `;` or a `(` to the surrounding program.

```slurp
<div x-init="n = ${ v }"></div>
```

renders `x-init="n = &#34');alert(1);('&#34"`, which the browser decodes to `n = "');alert(1);('"`.

Opt-outs exist because a JS-evaluated attribute carries different kinds of value and only the author can tell them apart. `| js` declares "this is data inside a string literal"; `| unsafe_js` declares "this is JavaScript the theme itself wrote" (a prop composed into a handler, which escaping would corrupt); `| json` declares an already-encoded value. A declared slot is passed through unescaped.

**A false `| js` declaration is a compile error**, because a declared slot is emitted verbatim, so a wrong claim is strictly worse than no claim:

```slurp
<div x-init="n = ${ v | js }"></div>
```

```
error[UnsafeScriptInterpolation]: `| js` declares that this slot sits inside a JavaScript
string literal, but in the JavaScript-evaluated attribute `x-init` it does not ...
```

That is `lib.rs` `check_js_slot_positions` at compile time, with `Renderer::escape_slot` as the render-time half. An *undeclared* slot in the same position is not an error: it becomes a JSON literal, and numeric slots such as `x-show="active == ${ loop.index }"` are ordinary and cannot be proved safe statically. In development mode the renderer warns on every undeclared JS slot (`warn_undeclared_js_slot`) so the author is told where to state intent; in production mode it is silent.

**3. Inside a `style` attribute or a bound `:style`.**

There is no useful *escape* for CSS: CSS escapes are only valid inside identifiers and strings, and a value slot in a style attribute can sit in any of a dozen grammatical positions. So `renderer::escape::escape_css_value` **removes** the structural characters instead - `;` `{` `}` `(` `)` `"` `'` `\` `@` `*` `<` `>` and NUL. `:` and `/` are deliberately kept: neither can open a new declaration on its own, and both are needed by the legitimate `background:url(${ path })` shape, while dropping `*` is enough to make `/*` comment injection impossible.

```slurp
<div style="background:url(${ c })">
```

With `c` set to `a);color:red;x:url(b` this renders `style="background:url(acolor:redx:urlb)"`. The injected declaration is gone.

`renderer::is_css_attr` routes `style`, `:style` and `x-bind:style`. CSS neutralisation runs *before* the JS pass in `escape_slot`, because doing it afterwards would strip the backslashes that pass just added. In development mode, a value the strip actually changed produces a warning (`warn_css_stripped`), since silently mangling a value is how a slot becomes a mystery bug.

**A `<style>` body is a different matter and is not covered.** `escape_css_value` is routed by attribute name; nothing applies it to text inside a `<style>` element. Do not interpolate untrusted data there.

**4. Inside `srcdoc`.**

`srcdoc` on an `<iframe>` holds an entire HTML document, so its content is entity-decoded once by the attribute parser and then parsed as HTML. `Renderer::srcdoc_guard` runs the same ammonia allowlist that `{html expr sanitize}` uses over the value, at the layer the iframe will actually see:

```slurp
<iframe srcdoc="${ d }"></iframe>
```

With `d` set to `<img src=x onerror=alert(1)><b>ok</b>` this renders `srcdoc='<img src="x"><b>ok</b>'`. The handler is gone and the markup survives. Sanitising rather than entity-escaping is what keeps `srcdoc` usable for the one thing it exists for.

The **bound** form (`:srcdoc` / `x-bind:srcdoc`) is deliberately not handled here: its value is a JavaScript expression evaluated in the browser, not markup, so the JS rules in point 2 cover it instead.

**A related trap, not a security issue but a silent one.** slurp interpolates `${ }` inside script bodies and attributes, so an ordinary JavaScript template literal can be consumed by the compiler. This is now diagnosed in both places:

```slurp
<script>let t = `${hours}h`;</script>
```

is the `<script>` case from point 1 and is a hard **compile error** - the message names template literals explicitly and tells you to build the string by concatenation. In an attribute:

```slurp
<a :href="`/cart/${item.id}`">
```

raises a **warning** (`JsTemplateLiteralInAttribute`, from `parser::html::warn_js_template_literal`). It is a warning rather than an error because slurp can legitimately interpolate a server value inside a backtick literal; the compiler cannot tell the author's client-side `item` from a server-side one, only that the shape is the one that goes wrong. The warning is deliberately suppressed when the backtick is unterminated, since a lone backtick is far more often ordinary prose. Note that `slurp validate` shows warnings only with `--warnings`.

#### Raw HTML Output

`{html expr}` emits the expression value without escaping. This is intentional for cases where the developer controls the HTML content (e.g., a rich-text body stored in a CMS field). It is the developer's responsibility to ensure the content is safe. **Do not use `{html}` with content that originates from end-user input.**

`{html expr sanitize}` runs the expression value through the `ammonia` crate before output. Ammonia operates on an **allowlist** of safe HTML elements and attributes, which is significantly safer than a blocklist approach because novel attack vectors are blocked by default.

**The allowlist is the contract.** Naming the tags that get stripped would be misleading, because anything absent from the list below is stripped, including tags that do not exist yet. `script`, `iframe`, `object`, `embed`, `applet`, `form`, `input`, `button`, `select`, `textarea`, `link`, `style`, `base` and `meta` are all absent from it, but they are not special cases.

**Allowed tags** (the tag allowlist in `renderer/sanitize.rs`):

`p` `br` `hr` `span` `div` `section` `article` `aside` `header` `footer` `main` `nav` `h1`-`h6` `ul` `ol` `li` `dl` `dt` `dd` `a` `strong` `em` `b` `i` `u` `s` `strike` `del` `ins` `mark` `sup` `sub` `small` `abbr` `cite` `code` `pre` `kbd` `samp` `var` `q` `blockquote` `img` `figure` `figcaption` `picture` `source` `table` `thead` `tbody` `tfoot` `tr` `th` `td` `caption` `colgroup` `col` `time` `data` `address` `details` `summary`

**Allowed attributes** (the attribute allowlist in `renderer/sanitize.rs`):

`class` `id` `title` `lang` `dir` `aria-label` `aria-describedby` `aria-hidden` `aria-live` `aria-atomic` `aria-relevant` `role` `href` `target` `src` `alt` `width` `height` `loading` `decoding` `sizes` `colspan` `rowspan` `scope` `headers` `datetime` `value` `open`

Every other attribute is dropped, which is how all `on*` event handlers are removed: not by a blocklist, but by never being allowed.

**Allowed URL schemes:** `http`, `https`, `mailto`. A `javascript:`, `data:` or `vbscript:` URL in any allowed URL attribute is removed with the attribute.

**Anchor hardening:** any `<a>` carrying a `target` has `rel="noopener noreferrer"` injected (the `link_rel` setting in `renderer/sanitize.rs`), so an opened page cannot reach back through `window.opener` and the referrer is not leaked.

The same sanitizer is what `Renderer::srcdoc_guard` runs over an `srcdoc` value, so everything in this subsection applies there too.

Weigh these before treating `sanitize` as sufficient:

- `img`, `source` and `picture` are allowed with `src`, so sanitized content can still load off-origin resources. (`srcset` is deliberately NOT allowed: ammonia's URL-scheme check does not parse a `srcset` candidate list, so a `data:text/html` candidate survived it. Dropping the attribute is what makes the scheme rule above true.) That is a tracking beacon: it leaks the reader's IP address and a referrer to whoever authored the content. If that matters, strip HTML entirely rather than sanitizing it.
- `class` and `id` are allowed, so sanitized content can adopt your stylesheet's classes and can collide with element IDs your own scripts query.

The `sanitize` flag is the right default when displaying user-generated rich text. It is not appropriate where no HTML at all should appear: use a plain `${ }` expression there.

---

### Environment Variables

**The compiler never reads the process environment.** It has no environment-variable lookup at all. `env` is an ordinary root in the render context, populated entirely by whatever host embeds the compiler, exactly like `site` or `product`. Understanding that is the whole of this section: the protection below is a check on a NAME written in a template, not a check on a value.

**What the compiler enforces.** A path whose root is `env` and whose second segment literally begins with `SLURP_SECRET_` is a compile error (`SecretEnvInTemplate`). The check is `check_env_key_name` in `lib.rs`, in the analysis pass, before any output is generated.

```slurp
${ env.SLURP_SECRET_STRIPE_KEY }   {* compile error *}
${ env["SLURP_SECRET_ANYTHING"] }  {* compile error - the subscript form too *}
${ (env)["SLURP_SECRET_X"] }       {* compile error - the parenthesised form too *}
```

**The refusal is unconditional, including inside middleware.** `check_env_key_name` takes no `is_middleware` argument and has no branch on it, so there is no file in which a `SLURP_SECRET_*` name is permitted. Middleware may read `env` (it is one of the three allowed roots), but not a key carrying that prefix.

Both spellings of the access are covered, and that is deliberate: `env.X` and `env["X"]` are the same access, and the prefix test once ran on the dotted form only. `check_expr`'s `Index` arm re-joins a parenthesised `(env)["..."]`, whose two halves would otherwise be checked independently with neither being a rooted `env.KEY` path. A subscript key that is **not** a compile-time constant is refused outright (`check_env_key_expr`), because there is no way to prove statically that `env[k]` is not a secret.

**What it does not enforce.** The match is on the literal prefix and nothing else. `${ env.STRIPE_KEY }` compiles cleanly, and renders whatever the host put at `context.env.STRIPE_KEY`. So this check catches a template author who names a secret in the obvious way; it cannot catch one who names it in any other way, and it cannot catch a host that seeded a secret into the context under a different key.

**The host's responsibility, and it is the load-bearing half.** Because the compiler cannot see your environment, the following is on the embedder, not on slurp:

1. **Do not put secrets in the render context.** The context is rendered into HTML. Anything in it can reach the page. The prefix check is a guard rail, not a boundary.
2. **If you populate `env` from the process environment, filter it.** Copy across an explicit allowlist of keys, or copy only keys carrying a prefix you have chosen as public. Never pass `std::env::vars()` through wholesale.
3. **Do not strip a prefix on the way in.** If you inject `SLURP_SECRET_FOO` into the context under the key `FOO`, you have disabled the check: `${ env.FOO }` is then a permitted path pointing at a secret. Keep the name the template writes identical to the name the check sees.

A convention is sometimes assumed where `SLURP_PUBLIC_FOO` is exposed to templates as `env.FOO` and `env.FOO` is resolved back to `SLURP_SECRET_FOO`. **No such mechanism exists**, in the compiler or the dev server. Do not rely on it, and note that implementing the prefix-stripping half of it without the resolution half is exactly the mistake described in point 3.

---

### Middleware Scope Restriction

Middleware files are permitted to inspect the incoming request and make routing decisions. To prevent middleware from being repurposed as a general-purpose data exfiltration layer, the compiler enforces that a path inside a middleware file may only have one of three roots (`check_path` in `lib.rs`):

- `request` - the incoming request, for example `request.cookies.session`, `request.headers`, `request.method`, `request.path`
- `env` - so a middleware can read configuration. Note this does **not** extend to `SLURP_SECRET_*`, which is refused everywhere; see the section above
- `loop` - the automatic loop variables, so `{each}` works inside a middleware

Any other path root is a compile error (`MiddlewareScopeViolation`). Note this is enforced on the ROOT only: the compiler does not restrict which fields of `request` a middleware reads, and the shape of `request` is supplied by the host. A subscript inside a path is itself an author-written expression and is checked in its own right, so `${ foo[request.cookies.session] }` does not slip past the guard by hiding inside an outer path.

Conversely, outside a middleware file:

- `request.*` is a compile error (`MiddlewareScopeViolation`, the `root == "request"` branch of `check_path`)
- `{redirect}` is a compile error (`RedirectOutsideMiddleware`, the `Node::Redirect` arm of `check_node`)
- `{next}` is a compile error (`RedirectOutsideMiddleware`, the `Node::Next` arm of `check_node`)

**"Middleware" is a caller-supplied flag, not a path convention.** It comes from `CompileOptions.is_middleware`. A host that sets that flag on the wrong file disables every rule in this section for it, and a host that never sets it makes middleware uncompilable. Nothing about a file's location or name makes it middleware.

---

### Image Optimizer Security

The image optimizer (`slurp-image`) is an HTTP microservice that fetches and transcodes remote images. It is a natural target for SSRF and resource exhaustion attacks.

#### SSRF Protection

The `src` URL parameter is validated before any network request is made:

1. The URL must parse as a valid URL (max length 4,096 bytes; no null bytes or control characters `< 0x20`).
2. Only `http://` and `https://` schemes are permitted. `file://`, `ftp://`, `gopher://`, `data:`, and all other schemes are rejected with a `DisallowedScheme` error.
3. Userinfo components (`user:pass@host`) are rejected to prevent credential injection.
4. After DNS resolution, **every resolved IP address** must be globally routable unicast before the connection is opened. This is an allowlist, not a blocklist, so any address space not explicitly routable fails closed:
   - IPv4 is accepted only in `1.0.0.0` - `223.255.255.255`, which excludes `0.0.0.0/8`, multicast `224.0.0.0/4`, reserved `240.0.0.0/4` and the `255.255.255.255` broadcast address outright.
   - Within that span these special-purpose blocks are rejected: `10.0.0.0/8`, `100.64.0.0/10` (CGNAT), `127.0.0.0/8` (loopback), `169.254.0.0/16` (link-local, and therefore cloud metadata), `172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`, `192.88.99.0/24` (6to4 anycast), `192.168.0.0/16`, `198.18.0.0/15` (benchmarking), `198.51.100.0/24` and `203.0.113.0/24`.
   - IPv6 is accepted only within global unicast `2000::/3`, which excludes `::1`, `::`, `fc00::/7` (ULA), `fe80::/10` (link-local), `ff00::/8` (multicast) and `100::/64`. Inside `2000::/3`, the documentation range `2001:db8::/32` and Teredo `2001::/32` are also rejected.
   - Addresses that embed an IPv4 target are unwrapped and checked as IPv4, because on a dual-stack or tunnelled host that is where the packet actually goes: IPv4-mapped `::ffff:a.b.c.d`, IPv4-compatible `::a.b.c.d`, NAT64 `64:ff9b::/96` and 6to4 `2002::/16`.

CNAME chains and hosts that resolve to multiple addresses are all checked; any single non-routable address in the resolution set causes the request to be aborted (`SsrfBlocked`).

**DNS rebinding is mitigated in code.** Validating the resolved addresses would be pointless if the HTTP client then performed its own second lookup when connecting, since that lookup can return a private address. The client is pinned to the exact addresses that just passed validation (`reqwest::ClientBuilder::resolve_to_addrs`, `processor.rs`), so the connection can only reach an address already checked. Redirects are refused outright (`redirect::Policy::none()`), which closes the other half of the same bypass.

#### Content-Length and Timeout

- The `Content-Length` response header is inspected before the download body is consumed. Any response claiming more than **20 MB** is rejected immediately (`SourceTooLarge`).
- The body is then streamed with a running total, and aborted the moment it exceeds the same cap. Content-Length can be absent or can lie, so the header check alone is not the limit.
- Remote fetch requests time out after **10 seconds**.
- A whole-request deadline (default **30 seconds**) is applied by a timeout layer, covering the transform as well as the fetch.

#### Decompression Bomb Protection

In this order:

1. **A pre-decode dimension check, which is the real defence.** The file header is parsed for its declared dimensions and colour type before any pixel buffer is allocated, and the request is rejected with `DecompressionBomb` if `width x height x bytes_per_pixel` exceeds **50 MB**. The per-pixel cost comes from the decoder's own colour type, so a 16-bit source is charged at its true 8 bytes per pixel. The arithmetic saturates, so dimensions near `u32::MAX` cannot wrap to a small product and slip through. A tiny file claiming 60000 x 60000 is refused here, having allocated nothing.
2. **An allocation limit handed to the decoder**, in case the header understates the real decode cost.

Separately, the **output** pixel count (default **8 MP**) is capped after accounting for the requested width, because a width cap alone bounds nothing on a very tall source and encode cost scales with pixels.

#### Magic Bytes Validation

The optimizer validates the **actual file header** (magic bytes), not the `Content-Type` response header or the file extension in the URL. A server returning `Content-Type: image/jpeg` with a file that does not begin with the JPEG magic bytes (`FF D8 FF`) will be rejected.

Decodable source formats and their required headers:

| Format | Magic Bytes |
|---|---|
| JPEG | `FF D8 FF` |
| PNG | `89 50 4E 47 0D 0A 1A 0A` |
| GIF87a | `47 49 46 38 37 61` |
| GIF89a | `47 49 46 38 39 61` |
| WebP | `52 49 46 46 .. .. .. .. 57 45 42 50` |

Any file whose header does not match one of these signatures is rejected (`InvalidMagicBytes`).

**These four decoders are the only ones linked into the binary.** `image/Cargo.toml` builds the crate with `default-features = false, features = ["jpeg", "webp", "png", "gif"]`. The default feature set would additionally link bmp, dds, exr, ff, hdr, ico, pnm, qoi, tga and tiff. That matters because format selection at decode time is by content, not by the header we matched: without the restriction, a file that satisfied the magic check could still be routed to a decoder this document never declared.

**A recognised header is not sufficient on its own.** After the magic check, the format the decoder infers from the content must equal the format the header declared, or the request is rejected. AVIF and HEIC are a deliberate special case: an ISO base media header (the `ftyp` marker at bytes 4 to 8, with brand `avif`, `avis`, `heic` or `mif1`) is recognised so that such a file is reported as an unsupported source rather than as corrupt data, but no ISO base media decoder is compiled in. AVIF is an **output** format only.

#### Path Traversal Prevention

The `src` parameter is parsed as a URL by the `url` crate. Path traversal sequences (e.g., `/../`, `%2F..%2F`) are normalised by the URL parser during validation. Additionally, `file://` and all non-HTTP schemes are blocked at the scheme-check step, so a `file:///etc/passwd` URL cannot reach the filesystem.

#### Rate Limiting

The image optimizer applies in-memory rate limiting of **100 requests per 60 seconds per client** (`RateLimiter::check_and_record` in `image/src/handler.rs`).

**The window is TUMBLING, not sliding.** The counter is hard-reset once the window has elapsed, so a client that empties its allowance at the end of one window and again at the start of the next can be admitted up to twice the nominal rate across the boundary. That is the standard fixed-window tradeoff and is accepted here, because this limiter is a last line of defence rather than the primary one.

An IPv6 client is bucketed by its `/64` rather than by its address (`ClientKey::V6Net64`), because a routine allocation is a whole `/64` and per-address buckets would let one client mint unlimited distinct keys. An IPv4-mapped IPv6 address collapses into the IPv4 bucket, so a dual-stack listener cannot yield a second allowance. The bucket table is sharded over 16 mutexes, capped per shard, and pruned on a background timer rather than on the request path; a poisoned mutex fails closed.

This limit resets on process restart and is not shared across multiple optimizer instances. For production deployments, rate limiting should additionally be enforced at the reverse proxy layer (e.g., Nginx `limit_req`, Cloudflare rate limiting) to survive restarts and horizontal scaling.

#### Resource Exhaustion

Decode, resize and encode are seconds of CPU work on a large image. They run on the blocking pool, never on an async worker, behind a global permit defaulting to one in-flight transform per core (`--max-concurrent-transforms`). Without both halves, enough concurrent large requests pin every async worker, at which point the process can no longer accept connections or run its own rate limiter.

#### Network Exposure Defaults

The service binds **loopback** by default and sends **no CORS headers** by default. Both are deliberate: it fetches attacker-supplied URLs, so exposing it should be an explicit act. Pass `--bind 0.0.0.0` to accept connections from the network and `--cors-allow-origin <origin>` (repeatable, or `*`) to enable CORS.

---

### Dev Server Security

The dev server (`slurp-dev`) is designed exclusively for local development and must not be exposed to the network.

#### Static File Serving - Path Traversal Prevention

Traversal is rejected in two places, because the dev server has two different ways of turning a URL into a filesystem path.

**First, at the top of request handling.** `security::request_path_is_safe` rejects the raw request path before any branch touches it: any `..` (ParentDir) component, a drive prefix such as `C:`, an absolute root, or an embedded NUL results in a 404. This guard is applied once, at the entry point, so every downstream branch inherits it - the static-file branch, the dynamic bracket-route branch, and the SSR branch. Percent-encoding is not a bypass: the HTTP layer has already decoded the path by the time this runs, so `%2e%2e` and `..%2f` are seen as the `..` they are.

**Second, per file.** Static file candidates go through `security::safe_path`, which:

1. Strips leading slashes to prevent absolute path interpretation.
2. Rejects `..` (ParentDir), drive-prefix and root components before canonicalization as an early defence-in-depth layer.
3. Calls `std::fs::canonicalize` on the joined path, resolving all symlinks and remaining traversal sequences. Canonicalization fails (returning `None`) if the path does not exist, resulting in a 404 rather than a server error.
4. Verifies that the canonical result has the canonical theme root as a prefix, comparing component-wise so that a sibling directory sharing a textual prefix cannot pass.

The dynamic bracket-route file and the SSR source file are not built by `safe_path`, so each is additionally re-checked with `security::is_contained` after canonicalization. The SSR check matters most: that branch does not merely serve bytes, it reads and compiles a `.slurp` file.

#### WebSocket (HMR) - Origin Restriction

The HMR WebSocket endpoint validates the `Origin` header. Only this dev server's own origin is accepted: `http://` on a loopback host (`localhost`, `127.0.0.1`, `::1`) at the port the server is listening on. A missing `Origin`, a `null` `Origin`, a foreign origin, and a loopback origin on a different port are all rejected with a 403.

The connection's peer address is also required to be loopback, but that check is defence in depth only and is **not** what prevents cross-site WebSocket hijacking. A malicious page runs in the developer's own browser, so its TCP peer is loopback like any other tab. WebSockets have no same-origin policy and send no preflight, so the `Origin` header - which is set by the browser and cannot be forged by page script - is the only check that actually works here.

This matters because the HMR feed is not empty: it carries changed page paths and build errors whose `file` field is an absolute local filesystem path.

#### DNS Rebinding - Host Restriction

A request carrying a `Host` header must name a loopback host, otherwise it is refused with a 403 that explains why. A request with NO `Host` header at all is allowed through, so that HTTP/1.0 clients still work; a browser always sends one, and a browser is the only thing a rebinding attack can drive. Binding to `127.0.0.1` is not sufficient on its own: in a DNS rebinding attack the attacker's own domain is made to resolve to `127.0.0.1`, at which point the browser treats the attacker's origin as same-origin with the dev server and every response becomes cross-origin readable. The `Host` header is the only place that lie is visible.

A developer who deliberately points a hosts-file entry at the dev server can opt that name in with `--allow-host <host>` (repeatable).

#### Backend Proxy

`/api/*` and `/auth/*` are proxied to `--backend-url` with the request's method, body, `Cookie` and `Authorization` forwarded. What keeps that from being a confused deputy for any page on the internet:

- A request carrying a foreign `Origin` is refused with a 403. An absent `Origin` is allowed, because a top-level navigation and a command-line client both send none. This is what stops a no-preflight cross-site `POST` with `credentials: 'include'` from landing a state-changing write on the developer's backend.
- The upstream's `access-control-allow-origin` response header is **not** relayed. That header describes the upstream's own CORS policy (dev backends commonly send `*`); replaying it here would hand a foreign page read access to responses this proxy fetched with the developer's cookies. `content-type`, `set-cookie`, `location` and `cache-control` are relayed.

Fixture responses (`--fixtures`) scope `access-control-allow-origin` to the requesting dev origin and add `Vary: Origin`. They never send `*`.

All HTTP methods are forwarded rather than restricted to a safe subset, deliberately: themes legitimately `POST`, `PUT` and `DELETE` against their backend during development, and a method allowlist would break that without adding protection the `Origin` and `Host` guards do not already provide.

#### `SLURP_MODE` Injection

The dev server injects `<script>window.SLURP_MODE = 'dev'</script>` into every page it serves (`dev-server/src/server.rs`, `HMR_INJECT_TEMPLATE`). It always writes `'dev'` and has no code path that writes any other value.

**It is the only producer of `window.SLURP_MODE` in this repository.** `slurp build` does not emit it, and there is no build pipeline here that sets `'production'`. A page built with `slurp build` and served by any host other than the dev server has `window.SLURP_MODE` undefined.

That matters because the runtime branches on this value. Anything reading it must treat an absent value as production, so that the safe behaviour is the default rather than something a host has to remember to opt into. A host that genuinely wants editor or dev behaviour must set the global itself, deliberately, before the runtime loads.

---

### Runtime Security

The runtime (`@slurp/runtime`) is an optional browser bundle. A page that uses no `{fetch}`, no `{$let}`, no `{try}` and no client navigation does not need it at all, and the whole of this subsection is then moot.

**No `eval()`** - The runtime contains no `eval()`, no `new Function()`, and no string-argument `setTimeout`. Verified by grep across `runtime/src`. It does, however, load Alpine.js, and **Alpine evaluates its own attribute values as JavaScript**. That is Alpine's design, not a slurp defect, but it is why the JS-evaluated-attribute case above is a real injection sink rather than a theoretical one.

**Fetch interceptor scope** - The interceptor is read from `window.SLURP_FETCH_INTERCEPTOR`, a global, not from an options object (`callFn` in `runtime/src/fetch/index.ts`). `SlurpRuntimeOptions.fetchInterceptor` still appears in `shared/interfaces.ts` but nothing in the repository reads or implements it; treat that type field as stale.

**The gate is an allowlist, which is the right direction.** The interceptor runs only when `window.SLURP_MODE` is `'dev'` or `'editor'`, so an absent or unrecognised mode means no interception. That matters because nothing in this repository ever sets a production value (see the `SLURP_MODE` subsection above): a blocklist keyed on `=== 'production'` would have made an undefined mode permissive on every built page, which is exactly the failure this shape avoids.

**DOM manipulation** - The runtime never uses `innerHTML` for content that came off the network. A client navigation parses the incoming document with `DOMParser`, then swaps in the parsed node with `replaceWith` (`doSwap` in `runtime/src/core/navigation.ts`); the new title is copied with `textContent`. The runtime contains no `innerHTML` write at all: the dev-mode error overlay builds every node with `createElement` and assigns every error field via `textContent` (`runtime/src/core/dev.ts`). `{html}` nodes are rendered by the Rust compiler at build time and never touched by the runtime.

**`cn()` class helper** - `cn()` is `twMerge(clsx(...))`. Neither library executes code or interprets strings as HTML; both return a plain class string.

**Not a security property, and do not use it as one:** `navigate=false` opts a link out of client navigation, and the compiler and runtime agree on the spelling. It is an author convenience for links that must be full page loads. It is not a boundary: it lives in markup the page itself controls, so nothing about it constrains an attacker.

---

### VSCode Extension / Language Server

The diagnostic provider parses `.slurp` files to provide real-time error highlighting. It calls the parse-only `validate` entry point (`vscode-extension/src/extension.ts`); `render` is never invoked from the extension. No expressions are evaluated and no network requests are made during parsing, so a malicious `.slurp` file opened in an editor cannot execute code through this path.

**Most WASM exports do not run the compile-time security checks, and exactly one does.**

- `parse`, `render`, `render_dev`, `validate`, `extract_schema` and `render_section` do **not** run the security walk. That is deliberate: they exist for browser and editor hosts where a preview is not a deploy, and the rules they skip are server-side concerns.
- **`validate_full(source, is_middleware)` runs it.** It is `validate` plus `slurp_compiler::check_security`, and it is a separate export rather than a change to `validate` precisely so the editor-preview hosts are unaffected while an authoring tool can get build parity.

The distinction matters because several rules live in the security walk and not in the parser: an unfiltered `${ }` in a `<script>` body, a false `| js` claim in JavaScript statement position, `env.SLURP_SECRET_*`, `request.*` outside middleware, and `{redirect}` / `{next}` outside middleware. **A tool that calls only `validate` will report a file clean that `slurp build` then rejects.** If you are integrating slurp into an authoring tool, publish gate or CI check, call `validate_full`.

At the call site:

- `is_middleware` must match the value the eventual compile will use, because the rules genuinely differ. A caller with no way to know passes `false`, which is what `compile`, `compile_with_registry` and the `slurp validate` CLI all do.
- The security walk returns on its **first** violation (it is a `Result`, not an accumulator), so at most one security diagnostic is appended per call, while parse diagnostics accumulate. Fix the reported one and re-run to see the next.

Rendering through WASM still enforces nothing: `render` and `render_dev` do not call the walk. The render-time backstops described earlier in this document (`render_script_expression`, `escape_slot`) do run there, so a `<script>` body slot and a false `| js` claim are still caught, but the middleware and secret-env rules are not. **Do not use a WASM render as the production render path.**

---

## The shared contract (`compiler/src/ast.rs`, `compiler/src/error.rs`, `shared/interfaces.ts`)

`compiler/src/ast.rs` and `compiler/src/error.rs` are the Rust definitions. `shared/interfaces.ts` is the TypeScript view of the same wire shapes, and is the only file in `shared/`; it is imported by the WASM package, the language server, the runtime, the Prettier plugin and the VS Code extension.

### Error codes

`compiler/src/error.rs` defines dedicated `ErrorCode` variants for security violations: `SecretEnvInTemplate`, `MiddlewareScopeViolation`, `RedirectOutsideMiddleware`, `UnsafeScriptInterpolation` and `JsTemplateLiteralInAttribute`. All of them are present in the TypeScript `SlurpErrorCode` union in `interfaces.ts`, so IDEs and language servers can surface them as first-class security diagnostics rather than generic parse errors.

The `file` field on `SlurpError` / `CompileError` is a plain `String` with no length limit or path sanitization. Diagnostics are shown in an IDE and on a terminal, not in rendered HTML, so this is not an XSS surface as used. It becomes one if a host serialises a diagnostic into a web response: the dev server's HMR `error` message carries `SlurpError[]` over WebSocket, and a consumer must render `file` and `message` as text, never as HTML.

### The AST

The sanitization decision is recorded at parse time, on `Node::RawHtml { expr, sanitize }`, so it cannot be lost in a later pass. The renderer checks the flag unconditionally when emitting raw HTML.

`AttributeValue::Interpolated` is likewise a parse-time record rather than a render-time guess. The parser builds it only from a quoted attribute string containing `${ }` slots, which is what lets `eval_interpolated` know there is author text around the data and read a quote context out of it. An `AttributeValue::Expr` carries no such surrounding text, so there is no quote context to read, and the renderer does **not** substitute a strict branch there: it evaluates the expression, runs the `srcdoc` guard, and applies `escape_attr` only. See "Attribute expressions are theme trust" under Known Limitations. That is a trust decision, not an escaping decision.

`ExprKind::Call` lets `f(a, b)` PARSE. It is not an execution surface: **there are no callable functions in slurp**, and the renderer's `ExprKind::Call` arm in `eval_expr` evaluates every call to `Value::Null`, so `${ Math.max(a, b) }` yields an empty string rather than a result or an error. If callable built-ins are ever added, the allowlist belongs in the security walk rather than in the AST. Until then, treat any tooling that suggests slurp has functions as wrong.

### `shared/interfaces.ts`

`SlurpRuntimeOptions.fetchInterceptor` declares `(fn, args) => Promise<unknown>`. **The type is stale: nothing reads it.** The live mechanism is the `window.SLURP_FETCH_INTERCEPTOR` global described under Runtime Security, and the field should be wired up or removed. An interceptor sees every fetch URL and could exfiltrate them, which is why the runtime admits one only in `dev` or `editor` mode.

`SerializedAST` is `Record<string, unknown>`, an opaque blob, so TypeScript consumers do not depend on the internal AST shape. A host that hands such a blob back to the compiler must treat it as untrusted input: a malformed AST is a route to a panic or to incorrect output.

---

## Known Limitations

- **An interpolated slot in JavaScript STATEMENT position is encoded, not escaped.** In a JS-evaluated attribute, a slot outside any string literal becomes a JSON literal. That is self-delimiting and cannot contribute structure to the surrounding program, which is the security property that matters, but it is not a general-purpose escape: the value's JavaScript *type* is whatever JSON gives it, so a string slot arrives quoted where the author may have expected a bare token. If you meant a bare value, say so with `| json`; if you meant theme-written code, say so with `| unsafe_js`.

- **`attr={ expr }` on a JS-evaluated attribute is theme trust, by design.** The unquoted expression form carries no surrounding author text, so there is no quote context to read, and the renderer does NOT substitute a strict branch: the value is emitted with `escape_attr` only, which is inert in an attribute the JS engine will evaluate. `srcdoc` is the one exception, guarded whoever composed it, because that value becomes a whole document. But a `{html attrs}` attribute **spread** is a different matter and is explicitly not a sanitizer for untrusted data: its whole purpose is forwarding Alpine directives, and Alpine evaluates the value of every directive and every bind as JavaScript. `x-show="alert(1)"`, `:title="alert(1)"` and `x-for="i in [alert(1)]"` all pass the name gate today, unavoidably - an allowlist that admits `:disabled="loading"` cannot also exclude `:title="alert(1)"`, because they are the same construct. What `is_safe_attr_name` buys is narrowing, not safety: it rejects native `on*` handlers (including via a `:onclick` / `x-bind:onclick` alias) and the four initialization-time Alpine sinks. **Never route untrusted data into an attribute spread.**

- **`srcset` scheme checking is partial.** `is_url_attr` lists `srcset`, but `has_forbidden_url_scheme` inspects only the leading scheme, and an `srcset` value is a comma-separated list of `url descriptor` pairs. A script scheme in the second or later candidate is not caught. Candidate URLs are fetched as images, so this is a resource-load question rather than a script-execution one; splitting `srcset` correctly means implementing its grammar, and that is recorded rather than half-implemented.

- **A `<style>` body has no CSS neutralisation.** `escape_css_value` is routed by attribute NAME (`is_css_attr` covers `style`, `:style`, `x-bind:style`), so nothing applies it to text inside a `<style>` element. Unlike the `<script>` case there is no compile-time refusal there either. Do not interpolate untrusted data into a `<style>` body.

- **`{html}` without `sanitize` trusts the theme developer.** If a developer passes user-generated content through `{html}` without `sanitize`, XSS is possible. This is an intentional trade-off; there is no way to detect this statically without understanding the data flow of the entire application.

- **The compile-time checks assume a trusted host.** `is_middleware` is a caller-supplied flag and `env` is a caller-supplied context root, so a host that mislabels a file or seeds a secret into the context defeats both checks without the compiler being able to notice. The embedding host is inside the trust boundary; the theme author is only partly inside it, per the trust levels at the top of this document.

- **Rendering is total, so most failures are silent.** Missing data yields `""`, budget overruns truncate and record a diagnostic, and unknown section settings and block types are dropped rather than rejected. This is deliberate for a multi-tenant service where a template must never take a page down, but it means a template can be badly wrong and still render a 200. Build with `--verbose` to see warnings; without it they are hidden.

- **Rate limiting is in-memory.** The image optimizer's rate limiter resets when the process restarts and is not shared between multiple instances. Production deployments must add rate limiting at the reverse proxy layer.

- **Dev server is not hardened for network exposure.** The `Origin`, `Host` and path-traversal guards are designed to protect against local threats (other browser tabs, a malicious page the developer visits, locally running malware). They are not a substitute for network-level access controls. The dev server binds to `127.0.0.1` only and offers no flag to widen that, which is deliberate.

- **The image optimizer has no source-domain allowlist.** It will fetch from any globally routable host. DNS rebinding itself is mitigated in code (see "SSRF Protection" above: the connection is pinned to the addresses that passed validation, and redirects are refused), so an RPZ resolver is not required for that. An explicit allowlist of permitted source domains is still worth adding where the set of sources is known, since it turns an open fetcher into a closed one.

- **Image cache entries are keyed by URL, not by response.** Two requests for the same `src`, width and format share a cache entry, so an upstream that varies its response by request header or by client will have only its first response cached and served to everyone for the TTL.
