#!/usr/bin/env bash
# Seed a TEST vault with realistic data covering every feature the Roostr UI
# renders, through the daemon API only (POST /api/mutate, POST /api/query).
#
#   GLON_DATA=/tmp/roostr-gpui-vault scripts/seed-test-vault.sh
#
# Env:
#   GLON_DATA   REQUIRED, must not be ~/.glon (the real vault). The service
#               token is read from $GLON_DATA/api-token.
#   ROOSTR_API  daemon base URL (default http://127.0.0.1:7333).
#
# Idempotent-ish: every object uses a fixed id (`create` with `id` is
# create-if-absent), blocks use fixed ids (`blocks_append` skips existing),
# tables/chat/reactions are only added when missing. A rerun adds nothing new.
#
# Needs curl, and jq (preferred) or python3 for reading JSON responses.
# Written for bash 3.2 (macOS /bin/bash) and newer.
set -eo pipefail

die() { printf 'seed: %s\n' "$*" >&2; exit 1; }

# ── Guard: never touch the real vault ─────────────────────────────────
[ -n "${GLON_DATA:-}" ] || die "GLON_DATA must be set explicitly to a TEST vault (refusing to guess; ~/.glon is the real vault)"
[ -d "$GLON_DATA" ] || die "GLON_DATA=$GLON_DATA is not a directory"
data_real=$(cd "$GLON_DATA" && pwd -P)
home_real=""
[ -d "$HOME/.glon" ] && home_real=$(cd "$HOME/.glon" && pwd -P)
if [ "$data_real" = "$home_real" ] || [ "$data_real" = "$HOME/.glon" ]; then
	die "GLON_DATA points at ~/.glon (the real vault) - refusing"
fi
[ -r "$GLON_DATA/api-token" ] || die "no readable $GLON_DATA/api-token"
TOKEN=$(tr -d '[:space:]' <"$GLON_DATA/api-token")
API=${ROOSTR_API:-http://127.0.0.1:7333}
API=${API%/}

command -v curl >/dev/null || die "curl is required"
if command -v jq >/dev/null; then
	JSONTOOL=jq
elif command -v python3 >/dev/null; then
	JSONTOOL=py
else
	die "jq or python3 is required"
fi

# ── JSON reading: jx '<jq filter>' '<python expr over d>' < json ──────
# Filters stay null-safe (`?`, `//`): macOS ships jaq as /usr/bin/jq, which
# errors on `null | .key` where jq yields null.
jx() {
	if [ "$JSONTOOL" = jq ]; then
		jq -r "$1"
	else
		python3 -c 'import json, sys
d = json.load(sys.stdin)
r = eval(sys.argv[1])
if r is None:
    pass
elif isinstance(r, bool):
    print("true" if r else "false")
elif isinstance(r, (list, tuple)):
    for x in r: print(x)
else:
    print(r)' "$2"
	fi
}

# Read a heredoc into a variable (bash 3.2-safe, unlike $(cat <<EOF) with parens).
j() { IFS= read -r -d '' "$1" || true; }

# ── Clock ─────────────────────────────────────────────────────────────
NOW_S=$(date +%s)
z=$(date +%z) # +HHMM / -HHMM
zs=${z:0:1}
zh=$((10#${z:1:2}))
zm=$((10#${z:3:2}))
TZOFF=$((zh * 60 + zm))
[ "$zs" = "-" ] && TZOFF=$((-TZOFF))
TZNAME=$(readlink /etc/localtime 2>/dev/null | sed 's|.*zoneinfo/||')
[ -n "$TZNAME" ] || TZNAME=UTC
MIDNIGHT_S=$((NOW_S - ((NOW_S + TZOFF * 60) % 86400 + 86400) % 86400))
# day N  → local noon N days from today, in ms
day() { echo $(((MIDNIGHT_S + $1 * 86400 + 12 * 3600) * 1000)); }

# ── HTTP ──────────────────────────────────────────────────────────────
FAILS_FILE=$(mktemp -t roostr-seed.XXXXXX)
trap 'rm -f "$FAILS_FILE"' EXIT
CREATED=""
EXISTED=""

http() { # http METHOD PATH [BODY]
	if [ -n "${3:-}" ]; then
		curl -sS -X "$1" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' --data-binary "$3" "$API$2"
	else
		curl -sS -X "$1" -H "Authorization: Bearer $TOKEN" "$API$2"
	fi
}

# mutate '<json object without now_ms>' → prints the response; returns 1 on failure (recorded).
mutate() {
	local body=$1 res ok err action
	body="${body%"${body##*[![:space:]]}"}" # rtrim
	body="${body%\}},\"now_ms\":$((NOW_S * 1000)),\"tz_offset_min\":$TZOFF}"
	res=$(http POST /api/mutate "$body") || {
		printf -- '- transport error: %s\n' "${body:0:120}" >>"$FAILS_FILE"
		return 1
	}
	ok=$(printf '%s' "$res" | jx '.ok // false' 'd.get("ok", False)')
	if [ "$ok" != true ]; then
		err=$(printf '%s' "$res" | jx '.error // "unknown error"' 'd.get("error", "unknown error")')
		action=$(printf '%s' "$body" | jx '.action' 'd["action"]')
		printf -- '- %s: %s  (%s)\n' "$action" "$err" "${body:0:160}" >>"$FAILS_FILE"
		printf 'seed: %s failed: %s\n' "$action" "$err" >&2
		return 1
	fi
	printf '%s' "$res"
}

query() { http POST /api/query "$1"; }
object() { http GET "/api/objects/$1"; }

# ── Value encoders ────────────────────────────────────────────────────
s() { printf '{"stringValue":"%s"}' "$1"; }
int() { printf '{"intValue":%s}' "$1"; }
bool() { printf '{"boolValue":%s}' "$1"; }
strs() { # strs a b c → valuesValue of stringValues
	local out="" x
	for x in "$@"; do out="$out${out:+,}{\"stringValue\":\"$x\"}"; done
	printf '{"valuesValue":{"items":[%s]}}' "$out"
}
links() { # links relationKey id...
	local key=$1 out="" x
	shift
	for x in "$@"; do out="$out${out:+,}{\"linkValue\":{\"targetId\":\"$x\",\"relationKey\":\"$key\"}}"; done
	printf '{"valuesValue":{"items":[%s]}}' "$out"
}
options() { # options "text:color" ... → relation options list
	local out="" x n=0 text color
	for x in "$@"; do
		text=${x%%:*}
		color=${x##*:}
		out="$out${out:+,}{\"mapValue\":{\"entries\":{\"id\":{\"stringValue\":\"opt-$n\"},\"text\":{\"stringValue\":\"$text\"},\"color\":{\"stringValue\":\"$color\"},\"orderId\":{\"stringValue\":\"$(printf '%06d' $n)\"}}}}"
		n=$((n + 1))
	done
	printf '{"valuesValue":{"items":[%s]}}' "$out"
}
# view config items
vfilter() { printf '{"mapValue":{"entries":{"key":{"stringValue":"%s"},"condition":{"stringValue":"%s"},"value":%s}}}' "$1" "$2" "$(strs "${@:3}")"; }
vsort() { printf '{"mapValue":{"entries":{"key":{"stringValue":"%s"},"type":{"stringValue":"%s"},"empty":{"stringValue":"end"}}}}' "$1" "$2"; }
vrel() { printf '{"mapValue":{"entries":{"key":{"stringValue":"%s"},"width":{"intValue":%s}}}}' "$1" "$2"; }
list() { # list item... (already-encoded ValueJSON items)
	local out="" x
	for x in "$@"; do out="$out${out:+,}$x"; done
	printf '{"valuesValue":{"items":[%s]}}' "$out"
}

# mk TEXT SUBSTRING TYPE [PARAM] → MarkJSON over the first occurrence (ASCII text)
mk() {
	local pre=${1%%"$2"*} p=""
	[ -n "${4:-}" ] && p=",\"param\":\"$4\""
	printf '{"from":%d,"to":%d,"type":%d%s}' "${#pre}" $((${#pre} + ${#2})) "$3" "$p"
}

# ── Object helpers ────────────────────────────────────────────────────
# create_obj ID TYPE NAME FIELDS_JSON_OBJECT_BODY (without braces)
create_obj() {
	local id=$1 type=$2 name=$3 fields=${4:-} res existed
	res=$(mutate "{\"action\":\"create\",\"id\":\"$id\",\"type_key\":\"$type\",\"name\":\"$name\",\"fields\":{$fields}}") || return 0
	existed=$(printf '%s' "$res" | jx '.existed // false' 'd.get("existed", False)')
	if [ "$existed" = true ]; then
		EXISTED="$EXISTED $id"
	else
		CREATED="$CREATED
- $id ($type) $name"
	fi
}

# blocks ID '<json array of blocks with parent>'
blocks() { mutate "{\"action\":\"blocks_append\",\"object_id\":\"$1\",\"blocks\":$2}" >/dev/null || true; }

# Text block JSON: tb ID PARENT STYLE TEXT [MARKS_JSON_ARRAY] [EXTRA_TEXT_PROPS] [EXTRA_BLOCK_PROPS]
tb() {
	local parent="" marks=${5:-[]} xt=${6:-} xb=${7:-}
	[ -n "$2" ] && parent=",\"parent\":\"$2\""
	printf '{"id":"%s"%s,"content":{"text":{"text":"%s","style":%d,"marks":%s%s}}%s}' "$1" "$parent" "$4" "$3" "$marks" "${xt:+,$xt}" "${xb:+,$xb}"
}
# Custom block JSON: cb ID PARENT CONTENT_TYPE META_JSON
cb() {
	local parent=""
	[ -n "$2" ] && parent=",\"parent\":\"$2\""
	printf '{"id":"%s"%s,"content":{"custom":{"contentType":"%s","meta":%s}}}' "$1" "$parent" "$3" "$4"
}
# Layout block JSON: lb ID PARENT STYLE
lb() {
	local parent=""
	[ -n "$2" ] && parent=",\"parent\":\"$2\""
	printf '{"id":"%s"%s,"content":{"layout":{"style":%d}}}' "$1" "$parent" "$3"
}
arr() { local out="" x; for x in "$@"; do out="$out${out:+,}$x"; done; printf '[%s]' "$out"; }

# Ensure an inline table exists (after block TARGET) and fill its cells.
# table_fill OBJECT TARGET_BLOCK ROWS COLS "r0c0|r0c1|..." "r1c0|..." ...
table_fill() {
	local obj=$1 target=$2 rows=$3 cols=$4 shape tid colline rowids r c cell text
	shift 4
	shape=$(object "$obj" | table_shape)
	if [ -z "$shape" ]; then
		mutate "{\"action\":\"table_create\",\"object_id\":\"$obj\",\"target_id\":\"$target\",\"position\":2,\"rows\":$rows,\"cols\":$cols}" >/dev/null || return 0
		shape=$(object "$obj" | table_shape)
	fi
	tid=$(printf '%s\n' "$shape" | sed -n 1p)
	colline=$(printf '%s\n' "$shape" | sed -n 2p)
	rowids=$(printf '%s\n' "$shape" | sed -n '3,$p')
	r=0
	for row in $rowids; do
		eval "line=\${$((r + 1)):-}"
		[ -n "$line" ] || break
		c=0
		for col in $colline; do
			text=$(printf '%s' "$line" | cut -d'|' -f$((c + 1)))
			cell="$row-$col"
			if [ "$r" = 0 ]; then
				mutate "{\"action\":\"block_update\",\"object_id\":\"$obj\",\"block_id\":\"$cell\",\"content\":{\"text\":{\"text\":\"$text\",\"style\":0,\"marks\":[{\"from\":0,\"to\":${#text},\"type\":0}]}}}" >/dev/null || true
			else
				mutate "{\"action\":\"block_update\",\"object_id\":\"$obj\",\"block_id\":\"$cell\",\"content\":{\"text\":{\"text\":\"$text\",\"style\":0}}}" >/dev/null || true
			fi
			c=$((c + 1))
		done
		if [ "$r" = 0 ]; then
			mutate "{\"action\":\"block_update\",\"object_id\":\"$obj\",\"block_id\":\"$row\",\"content\":{\"tableRow\":{\"isHeader\":true}}}" >/dev/null || true
		fi
		r=$((r + 1))
	done
	TABLES="$TABLES $obj:$tid"
}
TABLES=""

# stdin object JSON → line1 table id, line2 column ids, then row ids
table_shape() {
	jx '(.blocks | map({key: .id, value: .}) | from_entries) as $b
		| ([.blocks[] | select(.content.table != null)][0]) as $t
		| if $t == null then empty else
			($t.childrenIds | map($b[.])) as $kids
			| $t.id,
			  ([$kids[] | select(.content.layout.style == 5)][0].childrenIds | join(" ")),
			  ([$kids[] | select(.content.layout.style == 4)][0].childrenIds[])
		  end' \
		'(lambda b: (lambda t: [] if t is None else (lambda kids: [t["id"], " ".join(next(k for k in kids if k["content"].get("layout", {}).get("style") == 5)["childrenIds"])] + next(k for k in kids if k["content"].get("layout", {}).get("style") == 4)["childrenIds"])([b[c] for c in t["childrenIds"]]))(next((x for x in d["blocks"] if "table" in x["content"]), None)))({x["id"]: x for x in d["blocks"]})'
}

chat_count() {
	object "$1" | jx '[.blocks[] | select((.content.custom?.contentType? // "") == "chat")] | length' \
		'len([x for x in d["blocks"] if x["content"].get("custom", {}).get("contentType") == "chat"])'
}

options_count() {
	object "$1" | jx '.fields.options?.valuesValue?.items? // [] | length' \
		'len(d["fields"].get("options", {}).get("valuesValue", {}).get("items", []))'
}

chat_post() { # chat_post OBJECT TEXT [REPLY_TO] [AS_AUTHOR] → message id
	local extra=""
	[ -n "${4:-}" ] && extra=",\"as_author\":\"$4\""
	mutate "{\"action\":\"chat_post\",\"object_id\":\"$1\",\"text\":\"$2\",\"reply_to\":\"${3:-}\",\"thread_id\":\"\"$extra}" | jx '.id // empty' 'd.get("id")'
}

# ── Spaces ────────────────────────────────────────────────────────────
echo "seed: daemon $API, vault $GLON_DATA ($JSONTOOL)"
channels=$(http GET /api/channels)
S2_NAME="Garden Club"
# Space 1 is the vault's first space other than the seeded second one; a
# fresh vault has none, so it gets one (local mode never creates it itself).
S1=$(printf '%s' "$channels" | jx ".[] | select(.name != \"$S2_NAME\") | .id" "[c[\"id\"] for c in d if c[\"name\"] != \"$S2_NAME\"]" | sed -n 1p)
if [ -z "$S1" ]; then
	S1=$(mutate "{\"action\":\"channel_create\",\"name\":\"Roostr HQ\",\"icon\":\"🐓\"}" | jx '.id' 'd["id"]')
	[ -n "$S1" ] || die "could not create the first space"
	CREATED="$CREATED
- $S1 (channel) Roostr HQ"
fi
S2=$(printf '%s' "$channels" | jx ".[] | select(.name == \"$S2_NAME\") | .id" "[c[\"id\"] for c in d if c[\"name\"] == \"$S2_NAME\"]" | sed -n 1p)
if [ -z "$S2" ]; then
	S2=$(mutate "{\"action\":\"channel_create\",\"name\":\"$S2_NAME\",\"icon\":\"🌱\"}" | jx '.id' 'd["id"]')
	CREATED="$CREATED
- $S2 (channel) $S2_NAME"
fi
P1=${S1:0:8}
P2=${S2:0:8}
echo "seed: space 1 $S1, space 2 $S2"
CH1=$(printf '"channel":%s' "$(s "$S1")")
CH2=$(printf '"channel":%s' "$(s "$S2")")

# ── Relation options on bundled status/tag (only when still empty) ────
STATUS_OPTS=$(options "Backlog:grey" "Todo:blue" "In progress:yellow" "Review:purple" "Done:lime")
TAG_OPTS=$(options "roadmap:teal" "bug:red" "design:pink" "research:ice" "ops:orange" "garden:lime")
for P in "$P1" "$P2"; do
	if [ "$(options_count "bundled-rel-status-$P")" = 0 ]; then
		mutate "{\"action\":\"set_field\",\"object_id\":\"bundled-rel-status-$P\",\"key\":\"options\",\"value\":$STATUS_OPTS}" >/dev/null || true
	fi
	if [ "$(options_count "bundled-rel-tag-$P")" = 0 ]; then
		mutate "{\"action\":\"set_field\",\"object_id\":\"bundled-rel-tag-$P\",\"key\":\"options\",\"value\":$TAG_OPTS}" >/dev/null || true
	fi
done

# ── Custom relations (space 1) ────────────────────────────────────────
# rel ID KEY NAME FORMAT ICON MAXCOUNT OPTIONS_JSON [OBJECT_TYPES_JSON]
rel() {
	local ot=""
	[ -n "${8:-}" ] && ot=",\"object_types\":$8"
	create_obj "$1" relation "$3" "$CH1,\"key\":$(s "$2"),\"name\":$(s "$3"),\"format\":$(s "$4"),\"iconEmoji\":$(s "$5"),\"hidden\":$(bool false),\"readOnly\":$(bool false),\"maxCount\":$(int "$6"),\"options\":$7,\"bundled\":$(bool false)$ot"
}
EMPTY_OPTS=$(options)
PERSON_TYPE="bundled-type-person-$P1"
PROJECT_TYPE="bundled-type-project-$P1"
rel seed-rel-priority priority Priority status "🔥" 1 "$(options "Urgent:red" "High:orange" "Medium:yellow" "Low:teal")"
rel seed-rel-labels labels Labels tag "🔖" 0 "$(options "frontend:blue" "backend:purple" "docs:grey" "infra:orange" "quick win:lime")"
rel seed-rel-assignee assignee Assignee object "👤" 1 "$EMPTY_OPTS" "$(strs "$PERSON_TYPE")"
rel seed-rel-project project Project object "📁" 1 "$EMPTY_OPTS" "$(strs "$PROJECT_TYPE")"
rel seed-rel-estimate estimate Estimate number "⏱️" 0 "$EMPTY_OPTS"
rel seed-rel-kickoff kickoff Kickoff date "🚀" 0 "$EMPTY_OPTS"
rel seed-rel-approved approved Approved checkbox "☑️" 0 "$EMPTY_OPTS"
rel seed-rel-codename codename Codename shorttext "🏷" 0 "$EMPTY_OPTS"
rel seed-rel-notes notes Notes longtext "📝" 0 "$EMPTY_OPTS"
rel seed-rel-homepage homepage Homepage url "🌐" 0 "$EMPTY_OPTS"
rel seed-rel-contact_email contact_email "Contact email" email "✉️" 0 "$EMPTY_OPTS"
rel seed-rel-contact_phone contact_phone "Contact phone" phone "📞" 0 "$EMPTY_OPTS"

# ── People ────────────────────────────────────────────────────────────
person() { # person ID NAME ICON EMAIL PHONE URL DESCRIPTION TAG...
	local id=$1 name=$2 icon=$3 email=$4 phone=$5 url=$6 desc=$7
	shift 7
	create_obj "$id" person "$name" "$CH1,\"iconEmoji\":$(s "$icon"),\"email\":$(s "$email"),\"phone\":$(s "$phone"),\"url\":$(s "$url"),\"description\":$(s "$desc"),\"tag\":$(strs "$@")"
}
person seed-person-ada "Ada Lovelace" "👩‍💻" ada@example.com "+1 555 0100" https://en.wikipedia.org/wiki/Ada_Lovelace "Writes the algorithms. Owns the launch." research
person seed-person-grace "Grace Hopper" "🧭" grace@example.com "+1 555 0101" https://en.wikipedia.org/wiki/Grace_Hopper "Compiler whisperer; reviews every release." ops
person seed-person-linus "Linus Torvalds" "🐧" linus@example.com "+1 555 0102" https://kernel.org "Keeps the backend honest." bug
person seed-person-margaret "Margaret Hamilton" "🛰️" margaret@example.com "+1 555 0103" "https://en.wikipedia.org/wiki/Margaret_Hamilton_(software_engineer)" "Reliability and on-call." design research

# ── Project ───────────────────────────────────────────────────────────
create_obj seed-project-launch project "Roostr 1.0 launch" "$CH1,\"iconEmoji\":$(s "🚀"),\"status\":$(strs "In progress"),\"priority\":$(strs High),\"tag\":$(strs roadmap design),\"labels\":$(strs frontend backend),\"dueDate\":$(int "$(day 21)"),\"kickoff\":$(int "$(day -14)"),\"estimate\":$(int 120),\"approved\":$(bool true),\"codename\":$(s Sunrise),\"notes\":$(s "Ship the native client alongside the web app. Scope frozen two weeks before launch."),\"homepage\":$(s https://roostr.space),\"contact_email\":$(s launch@roostr.space),\"contact_phone\":$(s "+1 555 0199"),\"assignee\":$(links assignee seed-person-ada),\"description\":$(s "Everything needed to ship Roostr 1.0.")"

# ── Tasks ─────────────────────────────────────────────────────────────
# task ID NAME ICON STATUS PRIORITY DUE_DAY DONE ASSIGNEE ESTIMATE TAG...
task() {
	local id=$1 name=$2 icon=$3 status=$4 prio=$5 due=$6 done=$7 who=$8 est=$9
	shift 9
	create_obj "$id" task "$name" "$CH1,\"iconEmoji\":$(s "$icon"),\"status\":$(strs "$status"),\"priority\":$(strs "$prio"),\"dueDate\":$(int "$(day "$due")"),\"done\":$(bool "$done"),\"assignee\":$(links assignee "$who"),\"estimate\":$(int "$est"),\"project\":$(links project seed-project-launch),\"tag\":$(strs "$@")"
}
task seed-task-spec "Write the API spec" "📐" Done High -9 true seed-person-ada 5 docs research
task seed-task-editor "Port the block editor" "✍️" "In progress" Urgent 2 false seed-person-grace 13 roadmap
task seed-task-kanban "Kanban drag and drop" "🗂️" Todo Medium 5 false seed-person-linus 8 roadmap design
task seed-task-calendar "Calendar month view" "📅" Todo Low 9 false seed-person-margaret 5 design
task seed-task-sync "Fix sync stall on resume" "🐛" Review Urgent 1 false seed-person-linus 3 bug
task seed-task-icons "Icon pass for dark theme" "🎨" Backlog Low 16 false seed-person-margaret 2 design
task seed-task-release "Release checklist" "📦" Backlog High 20 false seed-person-grace 3 ops
task seed-task-docs "Publish user docs" "📚" Done Medium -3 true seed-person-ada 4 docs
task seed-task-recurring "Weekly review" "🔁" Todo Medium 0 false seed-person-ada 1 ops
# Recurring: every Monday and Thursday at 09:00 local (resets next occurrence on rerun).
mutate "{\"action\":\"repeat_set\",\"object_id\":\"seed-task-recurring\",\"rule\":{\"freq\":\"week\",\"interval\":1,\"weekdays\":[1,4],\"monthly\":\"date\",\"times\":[540],\"tz\":\"$TZNAME\"}}" >/dev/null || true

# Task body with relation blocks.
blocks seed-task-editor "$(arr \
	"$(tb te-desc "" 12 "Bring the web editor to GPUI with parity for every block style.")" \
	"$(cb te-rel-status "" relation '{"key":"status"}')" \
	"$(cb te-rel-priority "" relation '{"key":"priority"}')" \
	"$(cb te-rel-assignee "" relation '{"key":"assignee"}')" \
	"$(cb te-rel-due "" relation '{"key":"dueDate"}')" \
	"$(tb te-h "" 3 "Steps")" \
	"$(tb te-c1 "" 8 "Text styles" "[]" '"checked":true')" \
	"$(tb te-c2 "" 8 "Marks and mentions" "[]" '"checked":true')" \
	"$(tb te-c3 "" 8 "Tables")" \
	"$(tb te-c4 "" 8 "Drag to reorder")")"

# ── Bookmarks, note, file ─────────────────────────────────────────────
bookmark() { create_obj "$1" bookmark "$2" "$CH1,\"iconEmoji\":$(s "🔗"),\"url\":$(s "$3"),\"description\":$(s "$4"),\"tag\":$(strs "${@:5}")"; }
bookmark seed-bm-gpui "GPUI" https://www.gpui.rs "Fast, productive UI framework for Rust from the Zed team." research
bookmark seed-bm-anytype "Anytype" https://anytype.io "The everything app our object model borrows from." design
bookmark seed-bm-nostr "Nostr" https://nostr.com "Relays, keys and events: how spaces sync." research
bookmark seed-bm-zed "Zed editor" https://zed.dev "GPUI in production." research

create_obj seed-note-ideas note "Ideas inbox" "$CH1,\"iconEmoji\":$(s "💡"),\"tag\":$(strs research)"
blocks seed-note-ideas "$(arr \
	"$(tb ni-1 "" 6 "Offline-first search index")" \
	"$(tb ni-2 "" 6 "Share a query as a public page")" \
	"$(tb ni-3 "" 6 "Keyboard-only kanban")")"

# A File object needs real bytes from the harness/Blossom; this one only
# carries metadata so the file block and the File page have something to show.
create_obj seed-file-spec file "roostr-spec.pdf" "$CH1,\"file_mime\":$(s application/pdf),\"file_size\":$(int 48213),\"file_hash\":$(s 9f2c4e1ab3d07788e6f1c2b4a5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8)"

# ── Showcase page: every block style, mark, color, alignment ──────────
create_obj seed-page-showcase page "Editor showcase" "$CH1,\"iconEmoji\":$(s "🧪"),\"status\":$(strs "In progress"),\"tag\":$(strs design roadmap),\"codename\":$(s Prism),\"priority\":$(strs Medium),\"description\":$(s "Every block style the editor renders.")"
T1="Paragraph with bold, italic, strikethrough, underline, inline code, a link to gpui.rs and a mention of Ada Lovelace."
M1=$(arr "$(mk "$T1" bold 0)" "$(mk "$T1" italic 1)" "$(mk "$T1" strikethrough 2)" "$(mk "$T1" underline 3)" "$(mk "$T1" "inline code" 4)" "$(mk "$T1" gpui.rs 5 https://www.gpui.rs)" "$(mk "$T1" "Ada Lovelace" 8 seed-person-ada)")
T2="Colored words: purple text on a lime highlight, and bold italic together, plus a mention of the launch project."
M2=$(arr "$(mk "$T2" "purple text" 6 "#ab50cc")" "$(mk "$T2" "lime highlight" 7 "#2f4218")" "$(mk "$T2" "bold italic" 0)" "$(mk "$T2" "bold italic" 1)" "$(mk "$T2" "launch project" 8 seed-project-launch)")
T3="Second bullet with italic text"
M3=$(arr "$(mk "$T3" italic 1)")
blocks seed-page-showcase "$(arr \
	"$(tb sc-desc "" 12 "A tour of every block the editor renders.")" \
	"$(tb sc-h1 "" 1 "Heading one")" \
	"$(tb sc-p1 "" 0 "$T1" "$M1")" \
	"$(tb sc-p2 "" 0 "$T2" "$M2")" \
	"$(tb sc-h2 "" 2 "Heading two")" \
	"$(tb sc-quote "" 4 "Simplicity is prerequisite for reliability. - Edsger W. Dijkstra")" \
	"$(tb sc-h3 "" 3 "Heading three")" \
	"$(tb sc-code "" 5 'fn main() {\n    println!(\"hello, roostr\");\n}')" \
	"$(tb sc-b1 "" 6 "First bullet")" \
	"$(tb sc-b1a sc-b1 6 "Nested bullet")" \
	"$(tb sc-b1a1 sc-b1a 6 "Nested twice")" \
	"$(tb sc-b2 "" 6 "$T3" "$M3")" \
	"$(tb sc-n1 "" 7 "Numbered one")" \
	"$(tb sc-n2 "" 7 "Numbered two")" \
	"$(tb sc-n2a sc-n2 7 "Nested numbered")" \
	"$(tb sc-n3 "" 7 "Numbered three")" \
	"$(tb sc-c1 "" 8 "Checked checkbox" "[]" '"checked":true')" \
	"$(tb sc-c2 "" 8 "Unchecked checkbox")" \
	"$(tb sc-tog "" 10 "Toggle - open me")" \
	"$(tb sc-tog-a sc-tog 0 "Hidden paragraph inside the toggle.")" \
	"$(tb sc-tog-b sc-tog 8 "Hidden checkbox inside the toggle")" \
	"$(tb sc-callout "" 11 "Callout: this vault is seeded test data." "[]" "" '"backgroundColor":"#4c4523"')" \
	"$(tb sc-colors "" 3 "Colors and alignment")" \
	"$(tb sc-red "" 0 "Red text block" "[]" '"color":"#f55522"')" \
	"$(tb sc-bluebg "" 0 "Blue background block" "[]" "" '"backgroundColor":"#26304c"')" \
	"$(tb sc-center "" 0 "Centered paragraph" "[]" "" '"align":1')" \
	"$(tb sc-right "" 0 "Right aligned teal text" "[]" '"color":"#0fc8ba"' '"align":2')" \
	"$(tb sc-purplebg "" 6 "Bullet on a purple background" "[]" '"color":"#ecd91b"' '"backgroundColor":"#3e2a4a"')" \
	"$(cb sc-div1 "" divider '{"style":"line"}')" \
	"$(lb sc-row "" 0)" \
	"$(lb sc-col1 sc-row 1)" \
	"$(tb sc-col1-h sc-col1 3 "Left column")" \
	"$(tb sc-col1-t sc-col1 0 "Columns are a row layout holding column layouts.")" \
	"$(lb sc-col2 sc-row 1)" \
	"$(tb sc-col2-h sc-col2 3 "Right column")" \
	"$(tb sc-col2-t sc-col2 6 "Each column holds any blocks.")" \
	"$(cb sc-div2 "" divider '{"style":"dots"}')" \
	"$(tb sc-links "" 3 "Links, relations, bookmarks, files")" \
	"$(cb sc-link-card "" link '{"target":"seed-project-launch","style":"card"}')" \
	"$(cb sc-link-text "" link '{"target":"seed-task-editor"}')" \
	"$(cb sc-rel-status "" relation '{"key":"status"}')" \
	"$(cb sc-rel-tag "" relation '{"key":"tag"}')" \
	"$(cb sc-rel-codename "" relation '{"key":"codename"}')" \
	"$(cb sc-bookmark "" bookmark '{"url":"https://www.gpui.rs","title":"GPUI","description":"A fast, productive UI framework for Rust."}')" \
	"$(cb sc-file "" file '{"fileId":"seed-file-spec","hash":"9f2c4e1ab3d07788e6f1c2b4a5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8","mime":"application/pdf","name":"roostr-spec.pdf"}')" \
	"$(tb sc-table-h "" 3 "Inline table")" \
	"$(tb sc-end "" 0 "End of the showcase.")")"
table_fill seed-page-showcase sc-table-h 4 3 \
	"Block|Style id|Notes" \
	"Paragraph|0|Plain text with marks" \
	"Heading|1-3|Three sizes" \
	"Toggle|10|Children hidden until opened"

# ── Launch plan page: table + discussion ──────────────────────────────
create_obj seed-page-launch-plan page "Launch plan" "$CH1,\"iconEmoji\":$(s "🗺️"),\"tag\":$(strs roadmap),\"project\":$(links project seed-project-launch),\"assignee\":$(links assignee seed-person-ada)"
TL="Owner: Ada Lovelace. Tracks the Roostr 1.0 launch."
ML=$(arr "$(mk "$TL" "Ada Lovelace" 8 seed-person-ada)" "$(mk "$TL" "Roostr 1.0 launch" 8 seed-project-launch)")
blocks seed-page-launch-plan "$(arr \
	"$(tb lp-desc "" 12 "Milestones, owners and the open questions for 1.0.")" \
	"$(tb lp-owner "" 0 "$TL" "$ML")" \
	"$(tb lp-h-goals "" 2 "Goals")" \
	"$(tb lp-g1 "" 6 "Native client at parity with the web app")" \
	"$(tb lp-g2 "" 6 "Zero data loss across sync")" \
	"$(tb lp-g3 "" 6 "Docs for every view")" \
	"$(tb lp-h-sched "" 2 "Schedule")" \
	"$(tb lp-after "" 0 "Questions go in the discussion below.")")"
table_fill seed-page-launch-plan lp-h-sched 4 3 \
	"Milestone|Owner|Date" \
	"Beta cut|Grace|In 1 week" \
	"Release candidate|Linus|In 2 weeks" \
	"Launch|Ada|In 3 weeks"

PEER=5eed00c0ffee0042 # a second (fake) author id so the thread has two voices
if [ "$(chat_count seed-page-launch-plan)" = 0 ]; then
	m1=$(chat_post seed-page-launch-plan "Kicking off the 1.0 launch thread. Draft plan is in the page above.")
	m2=$(chat_post seed-page-launch-plan "Looks great. Can we move the beta cut to Thursday?" "" "$PEER")
	m3=$(chat_post seed-page-launch-plan "Yes, updated the schedule table." "$m2")
	m4=$(chat_post seed-page-launch-plan "Release notes live in the Release checklist task." "" "$PEER")
	chat_post seed-page-launch-plan "Thanks! I will review them tomorrow." "$m4" >/dev/null
	[ -n "$m1" ] && mutate "{\"action\":\"chat_react\",\"object_id\":\"seed-page-launch-plan\",\"message_id\":\"$m1\",\"emoji\":\"👍\"}" >/dev/null || true
	[ -n "$m3" ] && mutate "{\"action\":\"chat_react\",\"object_id\":\"seed-page-launch-plan\",\"message_id\":\"$m3\",\"emoji\":\"🎉\"}" >/dev/null || true
	[ -n "$m4" ] && mutate "{\"action\":\"chat_react\",\"object_id\":\"seed-page-launch-plan\",\"message_id\":\"$m4\",\"emoji\":\"👀\"}" >/dev/null || true
fi

# ── Templates ─────────────────────────────────────────────────────────
create_obj seed-tpl-project template "Project brief" "$CH1,\"target_type\":$(s "$PROJECT_TYPE"),\"iconEmoji\":$(s "📋"),\"status\":$(strs Backlog)"
blocks seed-tpl-project "$(arr \
	"$(tb tp-desc "" 12 "One-paragraph summary of the project.")" \
	"$(tb tp-h1 "" 2 "Goals")" \
	"$(tb tp-g1 "" 6 "")" \
	"$(tb tp-h2 "" 2 "Milestones")" \
	"$(tb tp-m1 "" 8 "")" \
	"$(tb tp-h3 "" 2 "Risks")" \
	"$(tb tp-r1 "" 11 "What could block this?" "[]" "" '"backgroundColor":"#4f3d20"')")"
create_obj seed-tpl-bug template "Bug report" "$CH1,\"target_type\":$(s "bundled-type-task-$P1"),\"iconEmoji\":$(s "🐛"),\"tag\":$(strs bug),\"priority\":$(strs High)"
blocks seed-tpl-bug "$(arr \
	"$(tb tb-h1 "" 3 "Steps to reproduce")" \
	"$(tb tb-s1 "" 7 "")" \
	"$(tb tb-h2 "" 3 "Expected")" \
	"$(tb tb-e "" 0 "")" \
	"$(tb tb-h3 "" 3 "Actual")" \
	"$(tb tb-a "" 0 "")")"
mutate "{\"action\":\"set_field\",\"object_id\":\"$PROJECT_TYPE\",\"key\":\"default_template_id\",\"value\":$(s seed-tpl-project)}" >/dev/null || true

# ── Queries (sets) and a collection ───────────────────────────────────
create_obj seed-q-tasks-table query "Open tasks" "$CH1,\"iconEmoji\":$(s "📋"),\"setOf\":$(strs task),\"viewType\":$(s table),\"viewFilters\":$(list "$(vfilter done notEqual true)" "$(vfilter priority notEmpty)"),\"viewSorts\":$(list "$(vsort dueDate asc)" "$(vsort priority desc)"),\"viewRelations\":$(list "$(vrel name 260)" "$(vrel status 130)" "$(vrel priority 110)" "$(vrel dueDate 130)" "$(vrel assignee 160)" "$(vrel tag 170)" "$(vrel estimate 90)" "$(vrel project 170)" "$(vrel done 70)")"
create_obj seed-q-tasks-kanban query "Task board" "$CH1,\"iconEmoji\":$(s "🗂️"),\"setOf\":$(strs task),\"viewType\":$(s kanban),\"viewGroupKey\":$(s status),\"viewSorts\":$(list "$(vsort priority asc)"),\"viewRelations\":$(list "$(vrel priority 120)" "$(vrel dueDate 120)" "$(vrel assignee 120)" "$(vrel tag 120)")"
create_obj seed-q-tasks-calendar query "Due calendar" "$CH1,\"iconEmoji\":$(s "📅"),\"setOf\":$(strs task project),\"viewType\":$(s calendar),\"viewDateKey\":$(s dueDate)"
create_obj seed-q-people-gallery query "People" "$CH1,\"iconEmoji\":$(s "👥"),\"setOf\":$(strs person),\"viewType\":$(s gallery),\"viewSorts\":$(list "$(vsort name asc)"),\"viewRelations\":$(list "$(vrel email 160)" "$(vrel phone 120)" "$(vrel tag 120)")"
create_obj seed-q-bookmarks query "Bookmarks" "$CH1,\"iconEmoji\":$(s "🔗"),\"setOf\":$(strs bookmark),\"viewType\":$(s gallery),\"viewRelations\":$(list "$(vrel url 200)" "$(vrel tag 120)")"
create_obj seed-col-reading collection "Reading list" "$CH1,\"iconEmoji\":$(s "📚"),\"collectionIds\":$(strs seed-bm-gpui seed-bm-anytype seed-bm-nostr seed-page-showcase seed-note-ideas seed-page-launch-plan),\"viewRelations\":$(list "$(vrel name 260)" "$(vrel url 220)" "$(vrel tag 160)")"

# ── Space 2 ───────────────────────────────────────────────────────────
create_obj seed-s2-page-welcome page "Welcome to the Garden Club" "$CH2,\"iconEmoji\":$(s "🌻"),\"tag\":$(strs garden)"
T4="Plant list lives in the Seed swap task. Ask Ada Lovelace about tomatoes."
M4=$(arr "$(mk "$T4" "Seed swap" 8 seed-s2-task-swap)" "$(mk "$T4" tomatoes 0)")
blocks seed-s2-page-welcome "$(arr \
	"$(tb s2w-desc "" 12 "A second space so the space switcher has something to switch to.")" \
	"$(tb s2w-h "" 1 "Season plan")" \
	"$(tb s2w-p "" 0 "$T4" "$M4")" \
	"$(tb s2w-c1 "" 8 "Order compost" "[]" '"checked":true')" \
	"$(tb s2w-c2 "" 8 "Fix the drip line")" \
	"$(tb s2w-q "" 4 "To plant a garden is to believe in tomorrow.")")"
create_obj seed-s2-task-swap task "Seed swap" "$CH2,\"iconEmoji\":$(s "🌱"),\"status\":$(strs Todo),\"dueDate\":$(int "$(day 6)"),\"tag\":$(strs garden)"
create_obj seed-s2-task-beds task "Build raised beds" "$CH2,\"iconEmoji\":$(s "🪵"),\"status\":$(strs "In progress"),\"dueDate\":$(int "$(day 12)"),\"tag\":$(strs garden ops)"
create_obj seed-s2-task-harvest task "Harvest party" "$CH2,\"iconEmoji\":$(s "🎃"),\"status\":$(strs Backlog),\"dueDate\":$(int "$(day 30)")"
create_obj seed-s2-q-tasks query "Garden tasks" "$CH2,\"iconEmoji\":$(s "🧺"),\"setOf\":$(strs task),\"viewType\":$(s kanban),\"viewGroupKey\":$(s status)"

# ── Pins (pin_order keys sort after the bundled "V") ──────────────────
pin() {
	mutate "{\"action\":\"set_field\",\"object_id\":\"$1\",\"key\":\"pin_order\",\"value\":$(s "$2")}" >/dev/null || true
}
pin seed-page-showcase k
pin seed-q-tasks-kanban m
pin seed-page-launch-plan o
pin seed-q-tasks-calendar q
pin seed-s2-page-welcome k
pin seed-s2-q-tasks m

# ── Report ────────────────────────────────────────────────────────────
echo
echo "seed: created:${CREATED:- (nothing new)}"
[ -n "$EXISTED" ] && echo "seed: already present:$EXISTED"
echo "seed: tables:${TABLES}"
echo "seed: file block note: seed-file-spec carries metadata only (no bytes - real files go through the harness/Blossom)."
if [ -s "$FAILS_FILE" ]; then
	echo "seed: FAILURES:" >&2
	cat "$FAILS_FILE" >&2
	exit 1
fi
echo "seed: done."
