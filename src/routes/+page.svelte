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
