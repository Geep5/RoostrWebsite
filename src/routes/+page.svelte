<script lang="ts">
	import LandingVeil from "$lib/components/LandingVeil.svelte";
</script>

<svelte:head>
	<link rel="icon" type="image/png" href="/favicon.png" />
	<meta property="og:image" content="/logo.png" />
	<title>Roostr — a central workplace for every kind of mind</title>
	<meta
		name="description"
		content="Roostr is a local-first workplace: your knowledgebase, worked by you and by the agents that live in it. One memory, shared — no account, no platform. Your key is your identity."
	/>
</svelte:head>

<main class="veil-page">
	<a class="veil-link" href="/app" aria-label="Enter Roostr">
		<LandingVeil />
	</a>

	<header class="chrome">
		<a class="mark" href="/" aria-label="Roostr home"><img src="/logo.png" alt="Roostr" /><span>Roostr</span></a>
		<nav class="chrome-links">
			<a class="login" href="/app">Log in</a>
			<a class="gh" href="https://github.com/Geep5/Roostr" aria-label="GitHub" title="GitHub">
				<svg viewBox="0 0 16 16" width="18" height="18" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg>
			</a>
		</nav>
	</header>

	<!-- The scene is the page; this is for screen readers and search. -->
	<div class="sr-only">
		<h1>Roostr — a central workplace for every kind of mind</h1>
		<p>
			Your knowledgebase, worked — by you, and by the agents that live in it. One memory, shared.
			No account, no platform. Your key is your identity.
		</p>
		<nav>
			<a href="/app">Open the app</a>
			<a href="/privacy">Privacy</a>
			<a href="https://github.com/Geep5/Roostr">GitHub</a>
		</nav>
	</div>
</main>

<style>
	:global(html),
	:global(body) {
		margin: 0;
		padding: 0;
	}
	.veil-page {
		position: relative;
		height: 100dvh;
		overflow: hidden;
		background: #06080f;
	}
	.veil-link {
		position: absolute;
		inset: 0;
		display: block;
		cursor: pointer;
	}
	.chrome {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		z-index: 3;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 18px 22px;
		pointer-events: none;
	}
	.chrome a {
		pointer-events: auto;
	}
	.mark {
		display: flex;
		align-items: center;
		gap: 9px;
		text-decoration: none;
		color: #aab1c6;
		font-size: 15px;
		font-weight: 500;
		letter-spacing: 0.04em;
	}
	.mark img {
		width: 26px;
		height: 26px;
		border-radius: 7px;
	}
	.mark:hover {
		color: #e3e8f4;
	}
	.chrome-links {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.login {
		padding: 7px 16px;
		border-radius: 8px;
		background: rgba(170, 177, 198, 0.1);
		border: 1px solid rgba(170, 177, 198, 0.22);
		color: #cfd5e4;
		font-size: 13px;
		font-weight: 500;
		text-decoration: none;
		backdrop-filter: blur(6px);
	}
	.login:hover {
		background: rgba(170, 177, 198, 0.18);
		color: #fff;
	}
	.gh {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 34px;
		height: 34px;
		border-radius: 8px;
		color: #8d93a8;
		background: rgba(170, 177, 198, 0.08);
		border: 1px solid rgba(170, 177, 198, 0.16);
	}
	.gh:hover {
		color: #fff;
		background: rgba(170, 177, 198, 0.16);
	}
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}
</style>
