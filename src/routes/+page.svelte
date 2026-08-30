<script lang="ts">
	// Icons lifted from the app itself (block drag handle, the dataview
	// layout/table icon, the graph glyph, the sort arrows) so the site
	// speaks the product's visual language.
	const features = [
		{
			icon: "editor",
			title: "A real block editor",
			body: "Paragraphs, headings, lists, checkboxes, tables, embeds. Type / for commands, paste a YouTube link and play it inline, drag blocks anywhere.",
		},
		{
			icon: "table",
			title: "Queries & collections",
			body: "Every note is an object with typed properties. Build live queries and collections with filters, sorts, and resizable columns — your notes become a database.",
		},
		{
			icon: "graph",
			title: "Graph view",
			body: "A GPU-rendered force graph of every object and link in a channel. Click a node to open it, drag to rearrange — nothing ever overlaps.",
		},
		{
			icon: "agent",
			title: "Agents that live in your notes",
			body: "Every channel can have its own AI agent with long-term memory, skills, and tools over your objects. Chat with it from any device — it answers from the machine it lives on.",
		},
		{
			icon: "sync",
			title: "Sync without a server",
			body: "Changes travel as end-to-end encrypted Nostr events over relays you choose. No account, no backend, no company in the middle — your key is your identity.",
		},
		{
			icon: "dag",
			title: "History that can't lie",
			body: "Every edit is a content-addressed change in an append-only DAG. Your full history is replayable, verifiable, and merges cleanly across devices.",
		},
	];

	import { onMount } from "svelte";

	/** Scroll-trigger: animations start when their section approaches. */
	let inview = $state<Record<string, boolean>>({});
	onMount(() => {
		const io = new IntersectionObserver(
			(entries) => {
				for (const e of entries) if (e.isIntersecting) inview[e.target.id] = true;
			},
			{ rootMargin: "0px 0px -15% 0px" },
		);
		for (const id of ["agents", "model"]) {
			const el = document.getElementById(id);
			if (el) io.observe(el);
		}
		return () => io.disconnect();
	});

	const steps = [
		{
			n: "01",
			title: "Everything is an object",
			body: "Notes, tasks, queries, agents — all objects made of blocks and typed properties, stored as signed changes on your disk. The files are the truth; no database to corrupt, no cloud to trust.",
		},
		{
			n: "02",
			title: "Relays are the mailbox",
			body: "Each change is encrypted to your own key and published to Nostr relays. Other devices holding your key pull, verify the content address, and replay. Relays store ciphertext; only you can read it.",
		},
		{
			n: "03",
			title: "Agents answer from home",
			body: "An agent runs on the machine you set it up on. Message it from your laptop, it replies through the relays — with its memory, skills, and your notes at hand. If its machine is asleep, your message simply waits.",
		},
	];
</script>

<svelte:head>
	<link rel="icon" type="image/png" href="/favicon.png" />
	<meta property="og:image" content="/logo.png" />
	<title>Roostr — local-first notes with agents, synced over Nostr</title>
	<meta
		name="description"
		content="Roostr is a local-first, Anytype-style notes app: block editor, queries, graph view, and per-channel AI agents — synced end-to-end encrypted over Nostr. No server, no account. Your key is your identity."
	/>
</svelte:head>

<div class="page">
	<nav>
		<span class="brand"><img class="logo" src="/logo.png" alt="" /> Roostr</span>
		<div class="links">
			<a href="#agents">Agents</a>
			<a href="#model">The model</a>
			<a href="#own">Own it</a>
			<a href="/app" class="gh">Open app</a>
			<a href="https://github.com/Geep5/Roostr">GitHub</a>
		</div>
	</nav>

	<header class="hero">
		<h1>Your work. Your agents.<br /><span class="accent">Nobody's platform.</span></h1>
		<p class="sub">
			Roostr is a decentralized home base — todos, CRM, projects, memory — where AI agents work
			<em>with</em> you on the same objects you do. No account. No server. Your key is your
			identity, your relays are yours to choose, and your agents live where your data does.
		</p>
		<div class="cta">
			<a class="btn primary" href="/app">Open Roostr Web</a>
			<a class="btn" href="#how">How it works</a>
		</div>
		<div class="shot-wrap">
			<img class="shot" src="/shot-editor.webp" alt="Roostr's block editor: a note with properties, a table, and a discussion" />
		</div>
	</header>


	<section id="agents" class:inview={inview.agents}>
		<h2>Agents are citizens, not features</h2>
		<p class="section-sub">
			Every space can host agents with their own chats, memory, and skills — a coordinator fronts
			your device, specialists spawn for tasks, and they talk to each other. Their work lands as
			objects you can see, edit, and own.
		</p>
		<div class="scene">
			<div class="scene-chat">
				<div class="sc-msg sc-1"><span class="sc-avatar you">Y</span><div><div class="sc-meta">You</div><div class="sc-text">Pull this week's signups into a CRM view</div></div></div>
				<div class="sc-typing"><span class="sc-avatar g">G</span><span class="dots"><i></i><i></i><i></i></span></div>
				<div class="sc-msg sc-2"><span class="sc-avatar g">G</span><div><div class="sc-meta">Gracie · coordinator</div><div class="sc-text">On it — Stats, grab the numbers</div></div></div>
				<div class="sc-msg sc-3"><span class="sc-avatar st">S</span><div><div class="sc-meta">Stats · spawned specialist</div><div class="sc-text">3 new this week, 2 churned</div></div></div>
				<div class="sc-msg sc-4"><span class="sc-avatar g">G</span><div><div class="sc-meta">Gracie</div><div class="sc-text">Built it — pinned to your space</div></div></div>
			</div>
			<div class="scene-rail">
				<div class="rail-label">Personal</div>
				<div class="rail-card">📊 CRM — this week<span class="rail-sub">query · type = Signup</span></div>
			</div>
		</div>
		<p class="scene-cap">A real flow: you ask, the coordinator delegates to a spawned specialist, and the result becomes a first-class object — pinned, synced, yours.</p>
	</section>

	<section id="model" class:inview={inview.model}>
		<h2>One system, everything in it</h2>
		<p class="section-sub">Notes, tasks, people, bookmarks, chats — the same kind of thing, wearing different shapes.</p>
		<div class="model-grid">
			<div class="m-panel">
				<h3>Everything is an object</h3>
				<div class="morph">
					<div class="morph-card">
						<span class="morph-icon"><span class="mi mi-note">📝</span><span class="mi mi-task">✅</span><span class="mi mi-bm">🔖</span></span>
						<span class="morph-name">Sunday ride</span>
						<span class="morph-check">✓</span>
						<span class="morph-url">strava.com/…</span>
					</div>
					<div class="morph-badges"><span class="mb b1">note</span><span class="mb b2">task</span><span class="mb b3">bookmark</span></div>
				</div>
				<p>Same object — the type is just its shape. Retype it and the anatomy follows.</p>
			</div>
			<div class="m-panel">
				<h3>Types bring properties</h3>
				<div class="explode">
					<div class="ex-layer l1"><span>Sunday ride</span><em>name</em></div>
					<div class="ex-layer l2"><span>🚴</span><em>icon</em></div>
					<div class="ex-layer l3"><span>In progress</span><em>status</em></div>
					<div class="ex-layer l4"><span>Fri</span><em>due</em></div>
					<div class="ex-layer l5"><span>→ You</span><em>assignee</em></div>
				</div>
				<p>Typed fields defined once on the type, valued per object — and shared across every type that wants them.</p>
			</div>
			<div class="m-panel">
				<h3>Links are first-class</h3>
				<svg class="bloom" viewBox="0 0 260 140">
					<line class="bl e1" x1="60" y1="40" x2="130" y2="80" />
					<line class="bl e2" x1="60" y1="40" x2="70" y2="110" />
					<line class="bl e3" x1="130" y1="80" x2="200" y2="50" />
					<line class="bl e4" x1="200" y1="50" x2="210" y2="110" />
					<circle class="bn n1" cx="60" cy="40" r="9" />
					<circle class="bn n2" cx="130" cy="80" r="9" />
					<circle class="bn n3" cx="70" cy="110" r="9" />
					<circle class="bn n4" cx="200" cy="50" r="9" />
					<circle class="bn n5" cx="210" cy="110" r="9" />
				</svg>
				<p>Every link is a property — the graph view is just your links, visualized.</p>
			</div>
			<div class="m-panel">
				<h3>Queries stay live</h3>
				<div class="q-demo">
					<div class="q-rule">type: Task · done: false</div>
					<div class="q-grid">
						<span class="q-chip">Buy basil</span>
						<span class="q-chip q-moving">Ship sync fix <i class="q-check">✓</i></span>
						<span class="q-chip q-new">Write release notes</span>
					</div>
					<div class="q-done">✓ Done — collection<span class="q-chip q-arrived">Ship sync fix</span></div>
				</div>
				<p>A collection is a pile you made. A query is a question that keeps asking itself — flip the data and membership moves on its own.</p>
			</div>
		</div>
	</section>

	<section id="how" class="how">
		<h2>How it works</h2>
		<div class="steps">
			{#each steps as s (s.n)}
				<div class="step">
					<span class="n">{s.n}</span>
					<h3>{s.title}</h3>
					<p>{s.body}</p>
				</div>
			{/each}
		</div>
		<p class="fineprint">
			Built on a content-addressed change-DAG (every change is <code>sha256</code>-identified and
			verified on import), NIP-44 encryption, and relays you pick — swap them anytime, self-host
			one, or run fully offline. The protocol has two independent implementations that replay
			byte-identically.
		</p>
	</section>

	<section id="own">
		<h2>Nobody's platform, verifiably</h2>
		<div class="own-grid">
			<div class="card"><span class="f-icon">🔑</span><h3>Your key is the account</h3><p>There is no signup, no email, no recovery flow to social-engineer. Hold the key, hold the vault — on any device.</p></div>
			<div class="card"><span class="f-icon">🔀</span><h3>Relays are swappable pipes</h3><p>Changes travel as end-to-end encrypted Nostr events. Relays store ciphertext only; swap them, add your own, or run offline and sync later.</p></div>
			<div class="card"><span class="f-icon">🧾</span><h3>Proof, not vibes</h3><p>Every change is content-addressed and replayed on import. Two devices can compare a vault fingerprint and prove they hold identical state — and deletes that should be final actually are.</p></div>
		</div>
	</section>

	<section class="stack">
		<h2>Boring where it counts</h2>
		<div class="stack-grid">
			<div>
				<h3>Native core</h3>
				<p>A single Odin binary owns storage, replay, and queries. Plain files on disk — delete the app, keep your data.</p>
			</div>
			<div>
				<h3>Open protocols</h3>
				<p>Protobuf changes, Nostr transport, NIP-44 encryption. Nothing proprietary between you and your notes.</p>
			</div>
			<div>
				<h3>Recoverable by design</h3>
				<p>Deletes are tombstones, edits are history, and any device with your key can rebuild everything from the relays.</p>
			</div>
		</div>
	</section>

	<footer>
		<span class="brand"><img class="logo" src="/logo.png" alt="" /> Roostr</span>
		<span class="muted">local-first · end-to-end encrypted · yours</span>
		<a href="https://github.com/Geep5/Roostr">GitHub</a>
	</footer>
</div>

<style>
	:global(body) {
		margin: 0;
		background: #1e1e20;
		color: #f5f5f7;
		font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", Inter, sans-serif;
		-webkit-font-smoothing: antialiased;
	}
	:global(*) {
		box-sizing: border-box;
	}
	.page {
		max-width: 1060px;
		margin: 0 auto;
		padding: 0 24px;
	}

	nav {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 22px 0;
	}
	.brand {
		font-weight: 700;
		font-size: 17px;
	}
	.links {
		display: flex;
		gap: 22px;
		align-items: center;
	}
	.links a {
		color: #98989d;
		text-decoration: none;
		font-size: 14px;
	}
	.links a:hover {
		color: #f5f5f7;
	}
	.links .gh {
		color: #fff;
		background: #0a84ff;
		padding: 6px 14px;
		border-radius: 6px;
		font-weight: 500;
	}

	.hero {
		text-align: center;
		padding: 72px 0 24px;
	}
	h1 {
		font-size: clamp(38px, 6vw, 62px);
		line-height: 1.05;
		letter-spacing: -0.02em;
		margin: 0 0 22px;
	}
	.accent {
		color: #0a84ff;
	}
	.sub {
		max-width: 640px;
		margin: 0 auto 32px;
		color: #98989d;
		font-size: 18px;
		line-height: 1.6;
	}
	.cta {
		display: flex;
		gap: 12px;
		justify-content: center;
		margin-bottom: 56px;
	}
	.btn {
		padding: 10px 24px;
		border-radius: 8px;
		text-decoration: none;
		font-weight: 500;
		font-size: 15px;
		color: #f5f5f7;
		background: #2b2b2e;
		border: 1px solid #45454a;
		box-shadow: 0 0.5px 1px rgb(0 0 0 / 0.12);
	}
	.btn:hover {
		background: #3a3a3e;
	}
	.btn.primary {
		background: #0a84ff;
		border-color: #0a84ff;
		color: #fff;
	}
	.btn.primary:hover {
		background: #3395ff;
	}

	.shot-wrap {
		border: 1px solid #45454a;
		border-radius: 12px;
		overflow: hidden;
		box-shadow: 0 18px 50px rgb(0 0 0 / 0.35);
		background: #2b2b2e;
	}
	.shot {
		width: 100%;
		display: block;
	}

	section {
		padding: 88px 0 0;
	}
	h2 {
		font-size: 32px;
		letter-spacing: -0.01em;
		margin: 0 0 34px;
		text-align: center;
	}

	.card {
		border: 1px solid #45454a;
		border-radius: 10px;
		padding: 22px;
		background: #2b2b2e;
	}
	.card h3 {
		margin: 12px 0 8px;
		font-size: 17px;
	}
	.card p {
		margin: 0;
		color: #98989d;
		font-size: 14px;
		line-height: 1.6;
	}

	.steps {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 28px;
	}
	.step .n {
		color: #0a84ff;
		font-family: ui-monospace, monospace;
		font-size: 13px;
	}
	.step h3 {
		margin: 10px 0 8px;
		font-size: 18px;
	}
	.step p {
		margin: 0;
		color: #98989d;
		font-size: 14.5px;
		line-height: 1.65;
	}
	.fineprint {
		margin: 40px auto 0;
		max-width: 720px;
		text-align: center;
		color: #6e6e73;
		font-size: 13px;
		line-height: 1.7;
	}
	.fineprint code {
		color: #98989d;
	}


	.stack-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
		gap: 16px;
	}
	.stack-grid > div {
		border: 1px solid #45454a;
		border-radius: 10px;
		padding: 22px;
		background: #2b2b2e;
	}
	.stack-grid h3 {
		margin: 0 0 8px;
		font-size: 16px;
	}
	.stack-grid p {
		margin: 0;
		color: #98989d;
		font-size: 14px;
		line-height: 1.6;
	}

	footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 72px 0 40px;
		margin-top: 88px;
		border-top: 1px solid #45454a;
		font-size: 14px;
	}
	footer a {
		color: #98989d;
	}
	.muted {
		color: #6e6e73;
	}
	.logo {
		width: 26px;
		height: 26px;
		border-radius: 7px;
	}
	.brand {
		display: inline-flex;
		align-items: center;
		gap: 8px;
	}
	.f-icon {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		height: 40px;
		border-radius: 10px;
		background: #3a3a3e;
		border: 1px solid #45454a;
		color: #f5f5f7;
	}
	.section-sub {
		max-width: 640px;
		margin: 0 auto 40px;
		text-align: center;
		color: #98989d;
		font-size: 16px;
		line-height: 1.6;
	}

	/* ── Agents scene: scripted chat, plays on scroll into view ──── */
	.scene {
		display: grid;
		grid-template-columns: 1.6fr 1fr;
		gap: 18px;
		max-width: 860px;
		margin: 0 auto;
	}
	.scene-chat {
		background: #2b2b2e;
		border: 1px solid #45454a;
		border-radius: 12px;
		padding: 18px;
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-height: 260px;
	}
	.sc-msg {
		display: flex;
		gap: 10px;
		opacity: 0;
		transform: translateY(8px);
	}
	.sc-avatar {
		width: 26px;
		height: 26px;
		border-radius: 50%;
		flex: none;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 11px;
		font-weight: 600;
		color: #fff;
	}
	.sc-avatar.you { background: #5e5ce6; }
	.sc-avatar.g { background: #0a84ff; }
	.sc-avatar.st { background: #7d7aff; }
	.sc-meta { font-size: 11px; color: #98989d; margin-bottom: 2px; }
	.sc-text { font-size: 14px; line-height: 1.45; }
	.sc-typing { display: flex; align-items: center; gap: 10px; opacity: 0; }
	.sc-typing .dots { display: inline-flex; gap: 4px; }
	.sc-typing i { width: 6px; height: 6px; border-radius: 50%; background: #98989d; animation: dot-b 1s infinite; }
	.sc-typing i:nth-child(2) { animation-delay: 0.15s; }
	.sc-typing i:nth-child(3) { animation-delay: 0.3s; }
	@keyframes dot-b { 50% { opacity: 0.25; transform: translateY(-2px); } }
	#agents.inview .sc-1 { animation: sc-in 0.5s 0.4s both; }
	#agents.inview .sc-typing { animation: sc-in 0.4s 1.1s both, sc-out 0.3s 2.4s forwards; }
	#agents.inview .sc-2 { animation: sc-in 0.5s 2.6s both; }
	#agents.inview .sc-3 { animation: sc-in 0.5s 4.2s both; }
	#agents.inview .sc-4 { animation: sc-in 0.5s 5.6s both; }
	@keyframes sc-in { to { opacity: 1; transform: none; } }
	@keyframes sc-out { to { opacity: 0; height: 0; margin: -12px 0 0; } }
	.scene-rail {
		background: #2b2b2e;
		border: 1px solid #45454a;
		border-radius: 12px;
		padding: 14px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.rail-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: #98989d; }
	.rail-card {
		background: #3a3a3e;
		border: 1px solid #45454a;
		border-radius: 10px;
		padding: 12px;
		font-size: 14px;
		font-weight: 600;
		opacity: 0;
		transform: translateX(-40px) scale(0.92);
	}
	.rail-sub { display: block; font-size: 11px; font-weight: 400; color: #98989d; margin-top: 3px; }
	#agents.inview .rail-card { animation: rail-in 0.55s 6.6s cubic-bezier(0.2, 0.9, 0.3, 1.2) both; }
	@keyframes rail-in { to { opacity: 1; transform: none; } }
	.scene-cap { max-width: 640px; margin: 18px auto 0; text-align: center; color: #98989d; font-size: 13px; line-height: 1.6; }

	/* ── The model panels ────────────────────────────────────────── */
	.model-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(400px, 1fr));
		gap: 16px;
	}
	.m-panel {
		background: #2b2b2e;
		border: 1px solid #45454a;
		border-radius: 12px;
		padding: 20px;
	}
	.m-panel h3 { margin: 0 0 14px; font-size: 16px; }
	.m-panel p { margin: 14px 0 0; color: #98989d; font-size: 13.5px; line-height: 1.6; }

	/* morph: note -> task -> bookmark */
	.morph-card {
		position: relative;
		background: #3a3a3e;
		border: 1px solid #45454a;
		border-radius: 10px;
		padding: 14px;
		display: flex;
		align-items: center;
		gap: 10px;
		height: 24px;
	}
	.morph-name { font-weight: 600; }
	.morph-icon { position: relative; width: 22px; height: 22px; }
	.mi { position: absolute; inset: 0; opacity: 0; }
	.morph-check { margin-left: auto; width: 18px; height: 18px; border-radius: 4px; border: 1px solid #45454a; background: #2b2b2e; color: #fff; font-size: 11px; display: flex; align-items: center; justify-content: center; opacity: 0; }
	.morph-url { font-size: 11px; color: #98989d; opacity: 0; }
	.morph-badges { display: flex; gap: 8px; justify-content: center; margin-top: 10px; }
	.mb { font-size: 11px; padding: 3px 10px; border-radius: 999px; border: 1px solid #45454a; color: #98989d; transition: all 0.3s; }
	#model.inview .mi-note { animation: mi 9s 0.3s infinite; }
	#model.inview .mi-task { animation: mi 9s 3.3s infinite; }
	#model.inview .mi-bm { animation: mi 9s 6.3s infinite; }
	@keyframes mi { 0% { opacity: 0; } 4%, 32% { opacity: 1; } 36%, 100% { opacity: 0; } }
	#model.inview .b1 { animation: mb 9s 0.3s infinite; }
	#model.inview .b2 { animation: mb 9s 3.3s infinite; }
	#model.inview .b3 { animation: mb 9s 6.3s infinite; }
	@keyframes mb { 0% { border-color: #45454a; color: #98989d; } 4%, 32% { border-color: #0a84ff; color: #0a84ff; } 36%, 100% { border-color: #45454a; color: #98989d; } }
	#model.inview .morph-check { animation: mcheck 9s infinite; }
	@keyframes mcheck { 33%, 60% { opacity: 1; } 0%, 30%, 63%, 100% { opacity: 0; } }
	#model.inview .morph-url { animation: murl 9s infinite; }
	@keyframes murl { 66%, 93% { opacity: 1; } 0%, 63%, 96%, 100% { opacity: 0; } }

	/* exploded properties */
	.explode { display: flex; flex-direction: column; gap: 6px; }
	.ex-layer {
		display: flex;
		justify-content: space-between;
		align-items: center;
		background: #3a3a3e;
		border: 1px solid #45454a;
		border-radius: 8px;
		padding: 8px 12px;
		font-size: 13px;
		opacity: 0;
		transform: translateY(10px);
	}
	.ex-layer em { font-style: normal; font-size: 11px; color: #98989d; font-family: ui-monospace, monospace; }
	#model.inview .l1 { animation: sc-in 0.4s 0.4s both; }
	#model.inview .l2 { animation: sc-in 0.4s 0.65s both; }
	#model.inview .l3 { animation: sc-in 0.4s 0.9s both; }
	#model.inview .l4 { animation: sc-in 0.4s 1.15s both; }
	#model.inview .l5 { animation: sc-in 0.4s 1.4s both; }

	/* graph bloom */
	.bloom { width: 100%; height: 160px; }
	.bn { fill: #3a3a3e; stroke: #45454a; stroke-width: 1.5; opacity: 0; }
	.bl { stroke: #45454a; stroke-width: 1.5; stroke-dasharray: 200; stroke-dashoffset: 200; }
	.bn.n4, .bn.n2 { stroke: #0a84ff; }
	#model.inview .n1 { animation: sc-in 0.4s 0.3s both; }
	#model.inview .e1 { animation: draw 0.6s 0.6s both; }
	#model.inview .n2 { animation: sc-in 0.4s 0.8s both; }
	#model.inview .e2 { animation: draw 0.6s 0.9s both; }
	#model.inview .n3 { animation: sc-in 0.4s 1.1s both; }
	#model.inview .e3 { animation: draw 0.6s 1.2s both; }
	#model.inview .n4 { animation: sc-in 0.4s 1.5s both; }
	#model.inview .e4 { animation: draw 0.6s 1.8s both; }
	#model.inview .n5 { animation: sc-in 0.4s 2s both; }
	@keyframes draw { to { stroke-dashoffset: 0; } }

	/* live query */
	.q-demo { display: flex; flex-direction: column; gap: 10px; }
	.q-rule { align-self: flex-start; font-size: 11px; font-family: ui-monospace, monospace; background: #3a3a3e; border: 1px solid #0a84ff; color: #0a84ff; border-radius: 999px; padding: 3px 10px; }
	.q-grid { display: flex; flex-direction: column; gap: 6px; min-height: 96px; }
	.q-chip { display: flex; align-items: center; justify-content: space-between; background: #3a3a3e; border: 1px solid #45454a; border-radius: 8px; padding: 7px 11px; font-size: 13px; }
	.q-check { font-style: normal; width: 16px; height: 16px; border: 1px solid #45454a; border-radius: 4px; font-size: 10px; display: inline-flex; align-items: center; justify-content: center; color: transparent; }
	.q-done { font-size: 12px; color: #98989d; display: flex; align-items: center; gap: 8px; min-height: 34px; }
	.q-arrived { opacity: 0; }
	#model.inview .q-moving { animation: qmove 6s 1s infinite; }
	#model.inview .q-moving .q-check { animation: qcheck 6s 1s infinite; }
	#model.inview .q-arrived { animation: qarrive 6s 1s infinite; }
	#model.inview .q-new { animation: qnew 6s 1s infinite; }
	@keyframes qcheck { 0%, 18% { background: transparent; color: transparent; } 24%, 100% { background: #0a84ff; border-color: #0a84ff; color: #fff; } }
	@keyframes qmove { 0%, 24% { opacity: 1; transform: none; } 34%, 88% { opacity: 0; transform: translateY(6px); } 94%, 100% { opacity: 1; transform: none; } }
	@keyframes qarrive { 0%, 28% { opacity: 0; } 36%, 88% { opacity: 1; } 96%, 100% { opacity: 0; } }
	@keyframes qnew { 0%, 40% { opacity: 0; transform: translateY(-6px); } 48%, 88% { opacity: 1; transform: none; } 96%, 100% { opacity: 0; } }

	/* own it */
	.own-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; }

	@media (max-width: 800px) {
		.scene { grid-template-columns: 1fr; }
		.model-grid { grid-template-columns: 1fr; }
	}
	@media (prefers-reduced-motion: reduce) {
		.sc-msg, .sc-typing, .rail-card, .ex-layer, .bn { opacity: 1 !important; transform: none !important; animation: none !important; }
		.bl { stroke-dashoffset: 0 !important; animation: none !important; }
		.mi-note, .b1 { opacity: 1 !important; }
	}
</style>
