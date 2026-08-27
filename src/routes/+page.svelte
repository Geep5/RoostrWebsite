<script lang="ts">
	const features = [
		{
			icon: "▤",
			title: "A real block editor",
			body: "Paragraphs, headings, lists, checkboxes, tables, embeds. Type / for commands, paste a YouTube link and play it inline, drag blocks anywhere.",
		},
		{
			icon: "⌗",
			title: "Queries & collections",
			body: "Every note is an object with typed properties. Build live queries and collections with filters, sorts, and resizable columns — your notes become a database.",
		},
		{
			icon: "◬",
			title: "Graph view",
			body: "A GPU-rendered force graph of every object and link in a channel. Click a node to open it, drag to rearrange — nothing ever overlaps.",
		},
		{
			icon: "🤖",
			title: "Agents that live in your notes",
			body: "Every channel can have its own AI agent with long-term memory, skills, and tools over your objects. Chat with it from any device — it answers from the machine it lives on.",
		},
		{
			icon: "⇄",
			title: "Sync without a server",
			body: "Changes travel as end-to-end encrypted Nostr events over relays you choose. No account, no backend, no company in the middle — your key is your identity.",
		},
		{
			icon: "⛁",
			title: "History that can't lie",
			body: "Every edit is a content-addressed change in an append-only DAG. Your full history is replayable, verifiable, and merges cleanly across devices.",
		},
	];

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
	<title>Roostr — local-first notes with agents, synced over Nostr</title>
	<meta
		name="description"
		content="Roostr is a local-first, Anytype-style notes app: block editor, queries, graph view, and per-channel AI agents — synced end-to-end encrypted over Nostr. No server, no account. Your key is your identity."
	/>
</svelte:head>

<div class="page">
	<nav>
		<span class="brand">🐓 Roostr</span>
		<div class="links">
			<a href="#features">Features</a>
			<a href="#how">How it works</a>
			<a href="#agents">Agents</a>
			<a href="https://github.com/Geep5/Roostr" class="gh">GitHub</a>
		</div>
	</nav>

	<header class="hero">
		<h1>Your notes. Your machines.<br /><span class="accent">Nobody's server.</span></h1>
		<p class="sub">
			Roostr is a local-first knowledge base with a block editor, live queries, a graph view, and
			AI agents that live inside your notes — synced between your devices as end-to-end encrypted
			Nostr events. No account. No backend. Your key is your identity.
		</p>
		<div class="cta">
			<a class="btn primary" href="https://github.com/Geep5/Roostr">Get Roostr</a>
			<a class="btn" href="#how">How it works</a>
		</div>
		<div class="shot-wrap">
			<img class="shot" src="/shot-editor.webp" alt="Roostr's block editor: a note with properties, a table, and a discussion" />
		</div>
	</header>

	<section id="features">
		<h2>Everything a second brain needs</h2>
		<div class="grid">
			{#each features as f (f.title)}
				<div class="card">
					<span class="f-icon">{f.icon}</span>
					<h3>{f.title}</h3>
					<p>{f.body}</p>
				</div>
			{/each}
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

	<section id="agents" class="agents">
		<div class="agents-copy">
			<h2>An agent in every channel</h2>
			<p>
				Give a channel an agent and it becomes a collaborator: it reads and writes your objects,
				pins durable facts and milestones to structured memory, loads skills you write as ordinary
				notes, and compacts its own conversation so it never forgets the plot.
			</p>
			<p>
				It runs on <em>your</em> hardware with <em>your</em> Claude plan or API key — sign in once
				from Settings. Message it from any synced device; replies ride the relays home.
			</p>
		</div>
		<div class="shot-wrap small">
			<img class="shot" src="/shot-graph.webp" alt="Roostr's graph view: objects in a channel as a force-directed graph" />
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
		<span>🐓 Roostr</span>
		<span class="muted">local-first · end-to-end encrypted · yours</span>
		<a href="https://github.com/Geep5/Roostr">GitHub</a>
	</footer>
</div>

<style>
	:global(body) {
		margin: 0;
		background: #0c0e12;
		color: #e6e8ec;
		font-family: -apple-system, "Segoe UI", Inter, Roboto, sans-serif;
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
		color: #9aa0ab;
		text-decoration: none;
		font-size: 14px;
	}
	.links a:hover {
		color: #e6e8ec;
	}
	.links .gh {
		color: #0c0e12;
		background: #e6e8ec;
		padding: 7px 14px;
		border-radius: 999px;
		font-weight: 600;
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
		color: #ffa02f;
	}
	.sub {
		max-width: 640px;
		margin: 0 auto 32px;
		color: #9aa0ab;
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
		padding: 12px 26px;
		border-radius: 10px;
		text-decoration: none;
		font-weight: 600;
		font-size: 15px;
		color: #e6e8ec;
		border: 1px solid #2a2e37;
	}
	.btn:hover {
		border-color: #4a4f5a;
	}
	.btn.primary {
		background: #ffa02f;
		border-color: #ffa02f;
		color: #14100a;
	}
	.btn.primary:hover {
		background: #ffb254;
	}

	.shot-wrap {
		border: 1px solid #23262e;
		border-radius: 14px;
		overflow: hidden;
		box-shadow: 0 30px 80px rgba(0, 0, 0, 0.55);
		background: #101216;
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

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(290px, 1fr));
		gap: 16px;
	}
	.card {
		border: 1px solid #23262e;
		border-radius: 14px;
		padding: 22px;
		background: #101216;
	}
	.f-icon {
		font-size: 22px;
		color: #ffa02f;
	}
	.card h3 {
		margin: 12px 0 8px;
		font-size: 17px;
	}
	.card p {
		margin: 0;
		color: #9aa0ab;
		font-size: 14px;
		line-height: 1.6;
	}

	.steps {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 28px;
	}
	.step .n {
		color: #ffa02f;
		font-family: ui-monospace, monospace;
		font-size: 13px;
	}
	.step h3 {
		margin: 10px 0 8px;
		font-size: 18px;
	}
	.step p {
		margin: 0;
		color: #9aa0ab;
		font-size: 14.5px;
		line-height: 1.65;
	}
	.fineprint {
		margin: 40px auto 0;
		max-width: 720px;
		text-align: center;
		color: #6d727d;
		font-size: 13px;
		line-height: 1.7;
	}
	.fineprint code {
		color: #9aa0ab;
	}

	.agents {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 44px;
		align-items: center;
	}
	.agents h2 {
		text-align: left;
	}
	.agents-copy p {
		color: #9aa0ab;
		line-height: 1.7;
		font-size: 15.5px;
	}
	.agents-copy em {
		color: #e6e8ec;
		font-style: normal;
		font-weight: 600;
	}
	.shot-wrap.small {
		box-shadow: 0 20px 50px rgba(0, 0, 0, 0.45);
	}
	@media (max-width: 800px) {
		.agents {
			grid-template-columns: 1fr;
		}
	}

	.stack-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
		gap: 16px;
	}
	.stack-grid > div {
		border: 1px solid #23262e;
		border-radius: 14px;
		padding: 22px;
	}
	.stack-grid h3 {
		margin: 0 0 8px;
		font-size: 16px;
	}
	.stack-grid p {
		margin: 0;
		color: #9aa0ab;
		font-size: 14px;
		line-height: 1.6;
	}

	footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 72px 0 40px;
		margin-top: 88px;
		border-top: 1px solid #1b1e24;
		font-size: 14px;
	}
	footer a {
		color: #9aa0ab;
	}
	.muted {
		color: #6d727d;
	}
</style>
