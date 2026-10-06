<svelte:head>
	<title>Guide — Roostr</title>
	<meta name="description" content="How Roostr works: objects, spaces, agents, repeating work, scores and logins - with best practices and step-by-step setups." />
</svelte:head>

<!--
	The people version of docs/roostr-guide.md (the Roostr repo), which agents
	read as the "Roostr Guide" Skill. Same facts, in clicks rather than tools:
	when one changes, change the other.
-->
<main class="page">
	<a class="home" href="/">← Roostr</a>
	<h1>How Roostr works</h1>
	<p class="updated">A guide to the ideas, the setup, and the habits that keep it working.</p>

	<nav class="toc" aria-label="On this page">
		<a href="#idea">The idea</a>
		<a href="#objects">Objects</a>
		<a href="#spaces">Spaces</a>
		<a href="#computers">Computers</a>
		<a href="#agents">Agents</a>
		<a href="#repeat">Repeating work</a>
		<a href="#jev">Jev Skills</a>
		<a href="#credentials">Logins</a>
		<a href="#practices">Best practices</a>
		<a href="#recipes">Recipes</a>
	</nav>

	<h2 id="idea">The idea</h2>
	<p>
		Everything in Roostr is an <b>object</b>: a note, a task, an email, a person, an agent, a computer,
		a login, a saved view. Each object has a <b>page</b> for its content and <b>Properties</b> (the right-hand
		pane) for its settings and facts. You and your agents work on the same objects and see the same
		thing - nothing an agent does is hidden from you. When something goes wrong, it shows on the object
		itself, in its <b>Error</b> property.
	</p>

	<h2 id="objects">Objects</h2>
	<ul>
		<li><b>Types</b> say what kind of object something is (Note, Task, Email, Human, Agent...). Task-like types get a Done checkbox.</li>
		<li><b>The page</b> is lines of text: paragraphs, headings, bullets, numbered lists, checkboxes, quotes, code, toggles, link cards and files. Type <code>/</code> for the menu; select lines to move, copy or delete them together.</li>
		<li><b>Properties</b> are named fields shared across a space - text, number, date, checkbox, status, tags, links to other objects. Add one with “+ Add property”.</li>
		<li><b>Templates</b> make new objects with properties and text already filled in. Each type can have a default template.</li>
		<li><b>Queries</b> are live, saved filters (“Emails not done”); <b>Collections</b> are hand-picked lists. Both show as tables or boards.</li>
		<li><b>The graph</b> shows how everything links together.</li>
		<li><b>Nothing is lost</b>: deleted objects go to the bin, and every change is kept in history.</li>
	</ul>

	<h2 id="spaces">Spaces</h2>
	<ul>
		<li>A <b>space</b> holds a set of objects with its own types, properties, templates and agents. Switch spaces on the far left.</li>
		<li>A space is private or shared with people you invite. Everything is end-to-end encrypted - only members can read it, not the relays it syncs through, and not us.</li>
		<li>Agents belong to a space too: an agent picker only offers the agents of the space you are in. If something seems missing, check which space you are in first.</li>
		<li>Deleting a space deletes everything in it, for everyone in it.</li>
	</ul>

	<h2 id="computers">Computers</h2>
	<p>
		Roostr stores and syncs your objects in any browser. To make things <i>happen</i> - agents answering,
		work repeating, scores filling in - Roostr needs a <b>computer</b>: a machine of yours running the Roostr
		engine and harness. Once installed it appears as a Computer object. A computer can also have extra
		software installed (for example a headless browser for reading web pages, or Google Workspace for
		email); its page shows what works there.
	</p>

	<h2 id="agents">Agents</h2>
	<h3>What an agent is made of</h3>
	<p>An agent is an object like any other. Its Properties are its whole configuration:</p>
	<ul>
		<li><b>System prompt</b> - who it is and how it works: its standing instructions.</li>
		<li><b>Model</b> - the AI model it runs on.</li>
		<li><b>Credentials</b> - the logins it may use, including its model key (for example an Anthropic key).</li>
		<li><b>Tools</b> - extra abilities, such as running commands on its computer or reading web pages.</li>
		<li><b>Skills</b> - instructions it can read when a job calls for them.</li>
		<li><b>Served by</b> - the computer it runs on. No computer, no agent: its Error property tells you.</li>
	</ul>

	<h3>An agent's life</h3>
	<ol>
		<li><b>Made</b> - with “+ New agent” in any agent picker (you name it right there) or New object → Agent. The space's Agent template fills in the rest.</li>
		<li><b>Placed</b> - set Served by to a computer. It starts there within seconds; change it and the agent moves.</li>
		<li><b>Invited</b> - add it to an object's <b>Agent</b> property. An agent only works on objects that list it.</li>
		<li><b>Woken</b> - @-mention it in an object's chat, or let a Repeat on that object fire. A message without an @ wakes nobody.</li>
		<li><b>Working</b> - it reads the object, uses its tools, and posts one answer in the chat. You see everything it changed.</li>
		<li><b>Remembering</b> - it keeps its own history and notes, so it gets better at your space over time.</li>
		<li><b>Retired</b> - clear Served by to pause it, or delete it. Its past messages stay.</li>
	</ol>
	<p>Agents on the same object can talk to each other: one tags the other with a question, and the answer lands in the same chat.</p>

	<h2 id="repeat">Repeating work</h2>
	<ul>
		<li><b>Repeat</b> (top of any object's Properties) runs it every few minutes, hours, days, weeks, months or years, on the days and times you pick.</li>
		<li>Each time, the object's agent gets the object's page as instructions. Write the steps there.</li>
		<li>A daily (or longer) repeat waits until that run is done - the agent marks it, or you tick Done. Minute and hour repeats finish by themselves.</li>
		<li><b>Check first</b> runs a small tool before each run, without AI. If it finds nothing new, the agent isn't woken at all - no cost, no noise. If it finds something, the agent gets that list along with its instructions.</li>
		<li>Every run is recorded; a failed run shows in the Error property until the next good one.</li>
	</ul>

	<h2 id="jev">Jev Skills: scores your agents fill in</h2>
	<ul>
		<li>A <b>Jev Skill</b> is a Skill with an <b>Answer</b> (Score, Choice or Yes or no). It is one question - for example a “Spam meter” from 1 to 10 - that your agent asks TypeSafe's Jev about objects. Jev is very fast and cheap and always says how sure it is.</li>
		<li><b>Its page is the question.</b> Write it as a sentence, then a numbered list of levels (for a score, lowest first), or a bulleted list of options (for a choice), or <code>Yes: …</code> / <code>No: …</code> lines (for yes/no).</li>
		<li>The answer goes into a property named after the Skill (or its <b>Writes to</b>).</li>
		<li>Give the Skill to an agent through its <b>Skills</b>, add a TypeSafe credential to its Credentials, and tell it when to use it - for example on a repeating task's page: “First, score every new email with your Spam meter skill.”</li>
		<li>Scores are normal properties, so you can sort, filter and build queries on them (“Spam meter 8 or more”). Click a score to see how sure Jev was, “Ask again”, or “Edit question →”.</li>
	</ul>

	<h2 id="credentials">Logins (credentials)</h2>
	<ul>
		<li>A <b>Credential</b> is one login: an API key, a bot token, a database, a Google account or a website sign-in.</li>
		<li>Its <b>Served by</b> computer keeps it and checks it; its Status says whether it still works.</li>
		<li>Only you can sign in: press <b>Connect</b> and the sign-in window opens on that computer. Agents can prepare a credential but never sign in for you.</li>
		<li>An agent can use a login only if it is in the agent's Credentials. Pasted keys are visible to the members of the credential's space.</li>
	</ul>

	<h2 id="practices">Best practices</h2>
	<h3>Designing</h3>
	<ul>
		<li><b>One job per agent.</b> A triage agent, a support agent, a writer - small, focused agents are more reliable and cheaper than one that does everything.</li>
		<li><b>Put the steps on the work.</b> A recurring task's page says what to do each time; the agent's system prompt says who it is.</li>
		<li><b>Make work into objects.</b> One object per email, ticket or lead, with properties for status and owner - then you can see, sort and fix it.</li>
		<li><b>Properties for decisions, pages for reading.</b> Anything you will filter or sort by belongs in a property.</li>
		<li><b>Templates for anything you make twice.</b></li>
		<li><b>End every automation in a saved view</b> you can open to check on it.</li>
	</ul>
	<h3>Keeping it cheap and calm</h3>
	<ul>
		<li><b>Check first</b> so agents only wake when there is something to do.</li>
		<li><b>Score with Jev, decide with the agent:</b> a Jev Skill scores many things at once; the agent acts on the scores.</li>
		<li><b>The slowest repeat that is fast enough</b> - hourly is usually plenty.</li>
		<li><b>Give only what is needed:</b> computer access and logins only to the agents that use them.</li>
	</ul>
	<h3>When something doesn't work</h3>
	<ul>
		<li><b>Read the Error property</b> - missing computers, failed runs, signed-out logins and broken tools all show there.</li>
		<li><b>Agent not answering?</b> Check its Served by, its model key in Credentials, that it is on the object's Agent list, and that you @-mentioned it.</li>
		<li><b>Try it on a throwaway object</b> before pointing a setup at real data.</li>
		<li><b>Look before you make</b> - search and check your saved views so you don't create a second copy of something.</li>
		<li><b>Name things for people.</b> Clear names on agents, properties and views are most of the documentation.</li>
	</ul>

	<h2 id="recipes">Recipes</h2>
	<h3>A task an agent does every morning</h3>
	<ol>
		<li>New object → Task. Write the steps on its page, including what “done” means.</li>
		<li>Add the agent to its Agent property.</li>
		<li>Set Repeat to every weekday at 9:00.</li>
		<li>Optional: a Check first tool, so the agent only wakes when there's something new.</li>
		<li>After the first run, check the run and the Error property.</li>
	</ol>
	<h3>An inbox that sorts itself</h3>
	<ol>
		<li>Add the mailbox as a Google account credential and press Connect.</li>
		<li>A Task “Check inbox” with a short Repeat and an import tool as Check first, so each new email becomes an Email object.</li>
		<li>Put a triage agent on the task, and write on the task's page how to triage (close spam, hand real questions to your support agent).</li>
		<li>Give the triage agent Jev Skills (Spam meter, Abuse) and start the task's page with “First, score the new emails”.</li>
		<li>Save a query “Emails not done” to see what's open.</li>
	</ol>
	<h3>Score things with a Jev Skill</h3>
	<ol>
		<li>Make a TypeSafe credential, paste your API key, and add it to the agent's Credentials.</li>
		<li>New object → Skill. Name it after what it measures (“Urgency”) and set Answer to Score.</li>
		<li>Write the question and a numbered list of levels on its page.</li>
		<li>Add the Skill to the agent's Skills, and say on its task's page when to run it.</li>
		<li>Build a query on the new property.</li>
	</ol>
	<h3>Give an agent a login</h3>
	<ol>
		<li>New object → Credential from the service's template; pick the computer that keeps it.</li>
		<li>Paste the key, or press Connect and sign in.</li>
		<li>Add the credential to the agent's Credentials.</li>
		<li>Say in the agent's instructions when to use it.</li>
	</ol>
	<h3>A new agent</h3>
	<ol>
		<li>“+ New agent” in any agent picker, and name it.</li>
		<li>Set Served by, and check its System prompt, Model and Credentials (it needs a model key).</li>
		<li>Add it to the objects it should work on, then @-mention it once to say hello.</li>
	</ol>

	<p class="foot">Agents in Roostr read the same guide, as the “Roostr Guide” skill.</p>
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
		margin: 0 0 20px;
		font-size: 15px;
	}
	.toc {
		display: flex;
		flex-wrap: wrap;
		gap: 6px 14px;
		padding: 12px 0 4px;
		border-top: 1px solid #333338;
		border-bottom: 1px solid #333338;
		font-size: 14px;
	}
	.toc a {
		text-decoration: none;
	}
	h2 {
		font-size: 22px;
		margin: 40px 0 8px;
		scroll-margin-top: 16px;
	}
	h3 {
		font-size: 16px;
		margin: 22px 0 4px;
		color: #f5f5f7;
	}
	p,
	li {
		color: #d6d6db;
	}
	li {
		margin: 4px 0;
	}
	b {
		color: #f5f5f7;
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
	.foot {
		margin-top: 48px;
		color: #9a9aa2;
		font-size: 14px;
	}
</style>
