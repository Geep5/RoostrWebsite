<script lang="ts">
	// Icons lifted from the app itself (block drag handle, the dataview
	// layout/table icon, the graph glyph, the sort arrows) so the site
	// speaks the product's visual language.

		import LandingVeil from "$lib/components/LandingVeil.svelte";

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
			<a href="/app" class="gh">Open app</a>
			<a href="https://github.com/Geep5/Roostr">GitHub</a>
		</div>
	</nav>

	<header class="hero3d">
		<LandingVeil />
		<div class="veil-copy">
			<span class="eyebrow">R O O S T R</span>
			<h1>A central workplace.<br /><span class="dim">For every kind of mind.</span></h1>
			<p class="sub">
				Your knowledgebase, worked — by you, and by the agents that live in it. One memory,
				shared. No account, no platform. Your key is your identity.
			</p>
			<div class="cta">
				<a class="btn primary" href="/app">Enter</a>
			</div>
		</div>
		<div class="base-links">
			<a href="/privacy">Privacy</a>
			<a href="https://github.com/Geep5/Roostr">GitHub</a>
			<span class="muted">local-first · end-to-end encrypted · yours</span>
		</div>
	</header>



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
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		z-index: 3;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 22px 0;
	}
	.logo {
		width: 22px;
		height: 22px;
		border-radius: 5px;
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

	.hero3d {
		position: relative;
		height: 100dvh;
		min-height: 560px;
		overflow: hidden;
		display: flex;
		align-items: center;
		justify-content: center;
		text-align: center;
	}
	.veil-copy {
		position: relative;
		z-index: 2;
		padding: 48px 40px;
		pointer-events: none;
		background: radial-gradient(ellipse 60% 65% at 50% 50%, rgba(3, 4, 10, 0.78) 0%, rgba(3, 4, 10, 0.35) 55%, transparent 75%);
	}
	.veil-copy .cta,
	.veil-copy a {
		pointer-events: auto;
	}
	.eyebrow {
		display: block;
		font-size: 12px;
		letter-spacing: 0.55em;
		color: #6b6f82;
		margin-bottom: 28px;
	}
	.dim {
		color: #4f5878;
	}
	.base-links {
		position: absolute;
		bottom: 22px;
		left: 0;
		right: 0;
		display: flex;
		justify-content: center;
		gap: 22px;
		font-size: 12px;
		letter-spacing: 0.08em;
		z-index: 2;
	}
	.base-links a {
		color: #6b6f82;
		text-decoration: none;
	}
	.base-links a:hover {
		color: #c7ccdf;
	}
	.base-links .muted {
		color: #43475a;
	}
	.hero3d h1 {
		font-size: clamp(44px, 7vw, 76px);
		font-weight: 300;
		line-height: 1.04;
		letter-spacing: -0.015em;
		margin: 0 0 26px;
		text-shadow: 0 2px 40px rgba(3, 4, 10, 0.9);
	}
	.hero3d .sub {
		max-width: 480px;
		margin: 0 auto 40px;
		color: #8d92a6;
		font-size: 16px;
		line-height: 1.7;
		font-weight: 300;
		text-shadow: 0 1px 24px rgba(3, 4, 10, 0.9);
	}
	h1 {
		font-size: clamp(38px, 6vw, 62px);
		line-height: 1.05;
		letter-spacing: -0.02em;
		margin: 0 0 22px;
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

</style>
