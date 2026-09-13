<svelte:head>
	<title>Privacy — Roostr</title>
	<meta name="description" content="What Roostr stores, where it goes, and who can see it." />
</svelte:head>

<main class="page">
	<a class="home" href="/">← Roostr</a>
	<h1>Privacy policy</h1>
	<p class="updated">Effective 13 September 2026</p>

	<p>
		Roostr is a local-first notes app. It is built so that the people who run it — including us —
		cannot read your notes. This page describes exactly what is stored, where it travels, and what
		anyone along the way can see. It applies to the Roostr apps for iOS, the web app at roostr.space,
		and the default relay we operate.
	</p>

	<h2>No accounts, no analytics</h2>
	<p>
		Roostr has no user accounts. Your identity is a cryptographic key pair that is generated on your
		device (or imported by you). We never receive the private key. The apps contain no analytics,
		advertising or crash-reporting SDKs and do not send usage data to us or to anyone else.
	</p>

	<h2>What stays on your device</h2>
	<ul>
		<li>Your private key — in the iOS Keychain, or in your browser's storage for the web app.</li>
		<li>Your notes, tasks, tables, spaces and chats, and their full edit history.</li>
		<li>Keys for shared spaces you own or have been invited to.</li>
		<li>A list of the relays you sync with.</li>
	</ul>
	<p>Deleting the app, or choosing “Log out” in it, removes all of this from the device.</p>

	<h2>What leaves your device</h2>
	<p>
		To sync between your devices and with people you share spaces with, Roostr publishes your
		changes to Nostr relays as signed events. Before anything is sent it is encrypted end-to-end
		(NIP-44) with a key that only your devices — and, for shared spaces, the members you invited —
		hold. The identifiers attached to those events are blinded hashes, not your object names or ids.
	</p>
	<p>A relay therefore sees:</p>
	<ul>
		<li>your public key (the identity every event is signed with),</li>
		<li>encrypted ciphertext and its size,</li>
		<li>timestamps, and</li>
		<li>the IP address your device connects from, as any internet server does.</li>
	</ul>
	<p>A relay cannot see note contents, titles, structure, or who you share a space with.</p>

	<h2>The default relay</h2>
	<p>
		New installs sync through <code>wss://roostr-relay.fly.dev</code>, which we operate on Fly.io.
		It stores the encrypted events it receives so your other devices can fetch them, and it keeps
		ordinary connection logs (IP address, time, request size) for up to 30 days to run and protect
		the service. We do not sell or share this data, and we cannot decrypt the events. You can point
		Roostr at any other relay, or run your own with the open-source
		<a href="https://github.com/Geep5/RoostrRelay">RoostrRelay</a>, at any time; the app then never
		contacts ours.
	</p>

	<h2>Sharing</h2>
	<p>
		When you invite someone to a space, the space key is sent to them inside an encrypted gift wrap
		addressed to their public key; when you request to join, your public key is sent to the space
		owner the same way. Members of a shared space can read everything in that space, and the owner
		can rotate its key to exclude a member from future changes.
	</p>

	<h2>Public profile</h2>
	<p>
		If you choose to set a display name or avatar, it is published as a standard Nostr profile
		event so that other members can recognise you. It is not encrypted and is visible to anyone who
		queries a relay for your public key. Leave it empty if you prefer.
	</p>

	<h2>The website</h2>
	<p>
		roostr.space is a static site served from Fly.io. Fly's proxy records standard access logs (IP
		address, time, path, user agent). The site sets no tracking cookies and loads no third-party
		scripts.
	</p>

	<h2>Children</h2>
	<p>Roostr is not directed at children under 13 and does not knowingly collect information from them.</p>

	<h2>Your control</h2>
	<p>
		Because your data is encrypted with your key and stored under your identity, there is nothing we
		can hand over, correct or delete on your behalf beyond the encrypted events on our relay. To
		remove those, delete the objects in the app or ask us to purge everything published under
		your public key.
	</p>

	<h2>Changes and contact</h2>
	<p>
		If this policy changes we will update the date above and note the change on this page.
		Questions: <a href="mailto:privacy@roostr.space">privacy@roostr.space</a>. The source of every
		Roostr component is public at <a href="https://github.com/Geep5">github.com/Geep5</a>.
	</p>
</main>

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
		max-width: 720px;
		margin: 0 auto;
		padding: 48px 24px 96px;
		line-height: 1.6;
	}
	.home {
		color: #9a9aa2;
		text-decoration: none;
		font-size: 14px;
	}
	h1 {
		font-size: 34px;
		margin: 24px 0 4px;
	}
	.updated {
		color: #9a9aa2;
		margin: 0 0 28px;
		font-size: 14px;
	}
	h2 {
		font-size: 20px;
		margin: 32px 0 8px;
	}
	p, li {
		color: #d6d6db;
	}
	a {
		color: #8ab4ff;
	}
	code {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 0.92em;
		background: #2a2a2e;
		padding: 1px 6px;
		border-radius: 4px;
	}
</style>
