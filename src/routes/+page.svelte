<script lang="ts">
	import { onMount } from "svelte";

	// The landing wears the logo cream wall-to-wall; the rest of the site
	// (the app!) keeps its own dark theme.
	onMount(() => {
		const prevBg = document.body.style.background;
		const prevTheme = document.querySelector('meta[name="theme-color"]')?.getAttribute("content") ?? null;
		document.body.style.background = "#EDCF7F";
		document.querySelector('meta[name="theme-color"]')?.setAttribute("content", "#EDCF7F");

		// Respect reduced-motion: swap the animated hero for its static poster.
		const hero = document.getElementById("rooster") as HTMLImageElement | null;
		if (hero && window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
			hero.src = "/roostr-poster.webp";
		}

		return () => {
			document.body.style.background = prevBg;
			if (prevTheme !== null) document.querySelector('meta[name="theme-color"]')?.setAttribute("content", prevTheme);
		};
	});
</script>

<svelte:head>
	<link rel="icon" type="image/png" href="/favicon.png" />
	<meta property="og:image" content="/logo.png" />
	<title>Roostr — your things speak</title>
	<meta
		name="description"
		content="Roostr gives every note, list and project its own agent. Local-first, offline-ready, synced through Nostr."
	/>
</svelte:head>

<main class="page">
	<div class="wrap">
		<header>
			<a class="wordmark" href="/">roostr<em>.</em></a>
			<a class="btn btn-ink btn-github" href="https://github.com/Geep5/Roostr" target="_blank" rel="noopener">
				<svg viewBox="0 0 16 16" width="17" height="17" aria-hidden="true" fill="currentColor">
					<path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/>
				</svg>
				GitHub
			</a>
		</header>

		<section class="hero">
			<div class="hero-copy">
				<span class="eyebrow">Every thing talks back</span>
				<h1>Your<br />things<br />speak<span class="dot">.</span></h1>
				<p class="lede">
					roostr gives every note, list and project its own agent. Talk to your
					things — they answer, remember, and act, always with your permission.
				</p>
				<div class="cta-row">
					<a class="btn btn-red" href="/app">Open web view</a>
				</div>
				<p class="meta">Local-first — runs offline, syncs via Nostr</p>
			</div>
			<div class="hero-art">
				<img
					id="rooster"
					src="/roostr-anim.webp"
					width="900"
					height="900"
					alt="The roostr rooster, gently swaying"
					fetchpriority="high"
				/>
			</div>
		</section>

		<section class="features" id="why">
			<div class="feature">
				<h2><span class="n">01</span>Everything answers</h2>
				<p>Every note, card and project has its own mailbox. Message one thing, or gather several into a group exchange.</p>
			</div>
			<div class="feature">
				<h2><span class="n">02</span>Your machines</h2>
				<p>Pair a machine with a one-use code and its agents join in. They act only after you approve, on that machine.</p>
			</div>
			<div class="feature">
				<h2><span class="n">03</span>Works offline</h2>
				<p>Identity lives in your browser. Changes queue in a durable outbox and sync through Nostr when you reconnect.</p>
			</div>
		</section>

		<footer>
			<span>© 2026 roostr</span>
			<span>made of objects</span>
		</footer>
	</div>
</main>

<style>
	@font-face {
		font-family: "Archivo Black";
		src: url("/fonts/ArchivoBlack.ttf") format("truetype");
		font-display: swap;
	}
	@font-face {
		font-family: "Inter";
		src: url("/fonts/Inter.ttf") format("truetype");
		font-weight: 100 900;
		font-display: swap;
	}

	.page {
		--cream: #EDCF7F;
		--ink: #1B1B1B;
		--red: #CA3433;
		--yellow: #FCD038;

		background: var(--cream);
		color: var(--ink);
		font-family: "Inter", system-ui, sans-serif;
		font-size: 17px;
		line-height: 1.55;
		-webkit-font-smoothing: antialiased;
		min-height: 100vh;
	}
	.page * {
		box-sizing: border-box;
	}
	h1,
	h2,
	p {
		margin: 0;
	}

	.wrap {
		max-width: 1200px;
		margin: 0 auto;
		padding: 0 32px;
	}

	/* ---------- header ---------- */
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 26px 0;
	}
	.wordmark {
		font-family: "Archivo Black", sans-serif;
		font-size: 26px;
		letter-spacing: -0.5px;
		text-decoration: none;
		color: var(--ink);
	}
	.wordmark em {
		font-style: normal;
		color: var(--red);
	}
	.btn {
		display: inline-block;
		text-decoration: none;
		cursor: pointer;
		font-weight: 650;
		font-size: 15px;
		padding: 12px 24px;
		border-radius: 999px;
		border: 2px solid var(--ink);
		transition:
			transform 0.15s ease,
			background 0.15s ease,
			color 0.15s ease;
	}
	.btn:hover {
		transform: translateY(-2px);
	}
	.btn:active {
		transform: translateY(0);
	}
	.btn-github {
		display: inline-flex;
		align-items: center;
		gap: 9px;
	}
	.btn-ink {
		background: var(--ink);
		color: var(--cream);
	}
	.btn-ink:hover {
		background: var(--red);
		border-color: var(--red);
	}
	.btn-red {
		background: var(--red);
		border-color: var(--red);
		color: #fff6e3;
	}
	.btn-red:hover {
		background: var(--ink);
		border-color: var(--ink);
	}

	/* ---------- hero ---------- */
	.hero {
		display: grid;
		grid-template-columns: 1.1fr 1fr;
		align-items: center;
		gap: 40px;
		padding: 48px 0 64px;
	}
	.eyebrow {
		display: inline-block;
		font-size: 13px;
		font-weight: 750;
		letter-spacing: 2.5px;
		text-transform: uppercase;
		color: var(--red);
		margin-bottom: 20px;
	}
	h1 {
		font-family: "Archivo Black", sans-serif;
		font-size: clamp(52px, 7.5vw, 96px);
		line-height: 0.98;
		letter-spacing: -1.5px;
		text-transform: uppercase;
		margin-bottom: 24px;
	}
	h1 .dot {
		color: var(--red);
	}
	.lede {
		font-size: 19px;
		max-width: 46ch;
		margin-bottom: 34px;
		font-weight: 450;
	}
	.cta-row {
		display: flex;
		gap: 14px;
		align-items: center;
		margin-bottom: 26px;
	}
	.meta {
		font-size: 14px;
		font-weight: 550;
		opacity: 0.75;
	}

	.hero-art {
		display: flex;
		justify-content: center;
	}
	.hero-art img {
		width: min(560px, 100%);
		height: auto;
		display: block;
	}

	/* ---------- features ---------- */
	.features {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 0;
		border-top: 2px solid var(--ink);
	}
	.feature {
		padding: 30px 28px 36px;
		border-left: 2px solid var(--ink);
	}
	.feature:first-child {
		border-left: none;
		padding-left: 0;
	}
	.feature h2 {
		font-family: "Archivo Black", sans-serif;
		font-size: 21px;
		text-transform: uppercase;
		letter-spacing: -0.3px;
		margin-bottom: 10px;
	}
	.feature h2 .n {
		color: var(--red);
		margin-right: 10px;
	}
	.feature p {
		font-size: 15.5px;
		opacity: 0.85;
	}

	/* ---------- footer ---------- */
	footer {
		border-top: 2px solid var(--ink);
		padding: 22px 0 30px;
		display: flex;
		justify-content: space-between;
		font-size: 14px;
		font-weight: 550;
		opacity: 0.8;
	}

	@media (max-width: 880px) {
		.hero {
			grid-template-columns: 1fr;
			padding-top: 24px;
		}
		.hero-art {
			order: -1;
		}
		.hero-art img {
			width: min(420px, 88%);
		}
		.features {
			grid-template-columns: 1fr;
		}
		.feature {
			border-left: none;
			padding-left: 0;
			border-top: 2px solid var(--ink);
		}
		.feature:first-child {
			border-top: none;
		}
	}
</style>
