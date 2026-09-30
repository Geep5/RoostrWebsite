<script lang="ts">
	import { onMount } from "svelte";

	// The landing wears the logo cream wall-to-wall; the rest of the site
	// (the app!) keeps its own dark theme.
	onMount(() => {
		const prevBg = document.body.style.background;
		const prevMargin = document.body.style.margin;
		// The browser's default 8px body margin would push a one-screen page into a scroll.
		document.body.style.margin = "0";
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
			document.body.style.margin = prevMargin;
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
		content="Roostr is a workspace where people and agents work as one team. No files, no folders: notes, tasks, people and agents are linked objects you shape yourself."
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
				<span class="eyebrow">People + agents, one workspace</span>
				<h1>Your<br class="wide" /> things<br /> speak<span class="dot">.</span></h1>
				<p class="lede">
					Notes, tasks, people and agents are linked objects in one graph — no files, no
					folders. Put an agent on anything and it works right there with you, in the open.
				</p>
				<div class="cta-row">
					<a class="btn btn-red" href="/app">Open Roostr</a>
					<span class="meta">Open source · no account · runs on your own machines</span>
				</div>
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
				<h2><span class="n">01</span>Objects, not files</h2>
				<p>Everything has properties and links. See any set as a table, board, gallery, calendar or graph. Files go peer to peer over WebRTC, coordinated by Roostr.</p>
			</div>
			<div class="feature">
				<h2><span class="n">02</span>Agents are objects</h2>
				<p>Set one up like anything else: a prompt, a model, skills, logins. Its logins are objects too, so it signs in on whichever of your computers runs it.</p>
			</div>
			<div class="feature">
				<h2><span class="n">03</span>One team</h2>
				<p><span class="mention">@mention</span> an agent in any chat and it answers there, or tags another agent in. Make a task repeat and its agent does it on schedule, following the page.</p>
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
		/* One screen, never a scroll: every size below scales with the window. */
		height: 100vh;
		height: 100svh;
		overflow: hidden;
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
		height: 100%;
		margin: 0 auto;
		padding: 0 32px;
		display: flex;
		flex-direction: column;
	}

	/* ---------- header ---------- */
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: clamp(12px, 2.6vh, 26px) 0;
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
		flex: 1;
		min-height: 0;
		display: grid;
		grid-template-columns: 1.1fr 1fr;
		align-items: center;
		gap: 40px;
		padding: clamp(6px, 1.6vh, 24px) 0;
	}
	.eyebrow {
		display: inline-block;
		font-size: 13px;
		font-weight: 750;
		letter-spacing: 2.5px;
		text-transform: uppercase;
		color: var(--red);
		margin-bottom: clamp(8px, 2vh, 20px);
	}
	h1 {
		font-family: "Archivo Black", sans-serif;
		font-size: clamp(36px, min(6.8vw, 9vh), 96px);
		line-height: 0.98;
		letter-spacing: -1.5px;
		text-transform: uppercase;
		margin-bottom: clamp(10px, 2.4vh, 24px);
	}
	h1 .dot {
		color: var(--red);
	}
	.lede {
		font-size: clamp(15px, 2.1vh, 19px);
		max-width: 46ch;
		margin-bottom: clamp(12px, 3vh, 30px);
		font-weight: 450;
	}
	.cta-row {
		display: flex;
		gap: 14px;
		align-items: center;
		flex-wrap: wrap;
	}
	.meta {
		font-size: 14px;
		font-weight: 550;
		opacity: 0.75;
	}

	.hero-art {
		height: 100%;
		min-height: 0;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.hero-art img {
		width: auto;
		height: auto;
		max-width: min(560px, 100%);
		max-height: 100%;
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
		padding: clamp(12px, 2.6vh, 30px) 28px clamp(12px, 2.8vh, 34px);
		border-left: 2px solid var(--ink);
	}
	.feature:first-child {
		border-left: none;
		padding-left: 0;
	}
	.feature h2 {
		font-family: "Archivo Black", sans-serif;
		font-size: clamp(16px, 2.3vh, 21px);
		text-transform: uppercase;
		letter-spacing: -0.3px;
		margin-bottom: clamp(4px, 1vh, 10px);
	}
	.feature h2 .n {
		color: var(--red);
		margin-right: 10px;
	}
	.feature p {
		font-size: clamp(13px, 1.75vh, 15.5px);
		opacity: 0.85;
	}
	/* The app's Discord-style mention pill (Discussion.svelte), with text
	   darkened for the cream background. */
	.mention {
		background: rgba(88, 101, 242, 0.3);
		color: #2f389e;
		border-radius: 3px;
		padding: 0 2px;
		font-weight: 500;
	}

	/* ---------- footer ---------- */
	footer {
		border-top: 2px solid var(--ink);
		padding: clamp(8px, 1.8vh, 20px) 0 clamp(10px, 2.4vh, 26px);
		display: flex;
		justify-content: space-between;
		font-size: 14px;
		font-weight: 550;
		opacity: 0.8;
	}

	/* Narrow screens stack: a small rooster on top, the copy, then the three
	   points as one tight list. Still one screen. */
	@media (max-width: 880px) {
		.wrap {
			padding: 0 20px;
		}
		header {
			padding: clamp(8px, 1.6vh, 16px) 0;
		}
		.btn {
			padding: 9px 18px;
			font-size: 14px;
		}
		.eyebrow {
			display: none;
		}
		/* "Your things / speak." - two lines instead of three. */
		h1 br.wide {
			display: none;
		}
		.lede {
			font-size: clamp(13.5px, 1.9vh, 16px);
			line-height: 1.45;
		}
		.cta-row {
			gap: 8px 14px;
		}
		.meta {
			font-size: 12px;
		}
		.hero {
			grid-template-columns: 1fr;
			grid-template-rows: minmax(0, 1fr) auto;
			gap: clamp(6px, 1.5vh, 16px);
			align-items: end;
		}
		.hero-art {
			order: -1;
		}
		h1 {
			font-size: clamp(28px, min(10vw, 6vh), 60px);
		}
		.features {
			grid-template-columns: 1fr;
		}
		.feature,
		.feature:first-child {
			border-left: none;
			padding: clamp(6px, 1.2vh, 12px) 0;
			border-top: 1.5px solid var(--ink);
		}
		.feature:first-child {
			border-top: none;
		}
		.feature h2 {
			font-size: clamp(14px, 2vh, 18px);
			margin-bottom: 2px;
		}
		.feature p {
			font-size: clamp(12px, 1.55vh, 14px);
			line-height: 1.35;
		}
		footer {
			font-size: 12px;
		}
	}
	/* Short phones: no room for the rooster - the copy is the page. */
	@media (max-width: 880px) and (max-height: 700px) {
		.hero-art {
			display: none;
		}
		.hero {
			grid-template-rows: auto;
			align-items: center;
		}
		.lede {
			font-size: 13px;
		}
		.feature p {
			font-size: 11.5px;
		}
	}
</style>
