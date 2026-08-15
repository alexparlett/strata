# The sample project's database half

A throwaway PostgreSQL with a seeded fixture, so the `sample/` project has something to federate
against. The project already carries the connection (`.strata/project.json`), so the only thing
between a checkout and a cross-source query is starting this container.

## Run it

Any container runtime serves — [colima](https://github.com/abiosoft/colima), Docker Desktop,
Rancher. With colima:

```bash
colima start
```

Then, from this folder:

```bash
docker build -t strata-sample-pg . && docker run -d --name strata-sample-pg -p 127.0.0.1:55432:5432 -e POSTGRES_USER=strata -e POSTGRES_DB=strata_sample -e POSTGRES_HOST_AUTH_METHOD=trust strata-sample-pg
```

`compose.yaml` does the same thing in one command if you have the Compose plugin (colima does not
install it):

```bash
docker compose up -d --build
```

Open `sample/` in Strata. The **pg** connection settles green on its own — there is no password to
enter.

Stop it, and take the data with it:

```bash
docker rm -f strata-sample-pg
```

The seed runs only on an empty data directory, which is why there is no volume: editing `init/`
and re-running the two commands above always gives you the fixture as written.

## What is in it

Five tables, a view and a materialized view in `public`, and two more tables in `analytics` — a
schema the connection deliberately does **not** display.

| Relation | Why it is there |
|---|---|
| `public.orders` | The headline. Nothing in the project is called `orders`, so `SELECT * FROM orders` resolves here with no qualifier. `tags` is `jsonb`, so `tags ->> 'channel'` pushes down to the server. |
| `public.shipments` | A second remote table, so a remote-to-remote join federates into one statement — visible in the plan view as a single `VirtualExecutionPlan`. |
| `public.support_tickets` | Joins to `users.parquet` on `user_id`. |
| `public.products` | **Deliberately clashes** with `products.parquet`. A bare `products` is always the project's own; this one is `pg.public.products`. |
| `public.sessions` | **Deliberately clashes** with `analytics.sessions`. Both schemas are shown, so a bare `sessions` is ambiguous and is refused by name — with both addresses in the message. |
| `public.open_tickets` | A view. Bare names resolve to views exactly as they do to tables. |
| `public.revenue_by_sku` | A materialized view, which the tree lists under Views and a bare name reaches. |
| `analytics.daily_revenue` | A second schema, so the tree has more than one to show and `daily_revenue` has somewhere to be. |
| `analytics.sessions` | The other half of the ambiguous name. |

Both schemas are enabled, and that is what makes the clash a clash: **an unqualified name searches
the schemas a connection shows**. Turn `analytics` off through *Schemas…* on the connection's node
and the rule is visible in one gesture — `sessions` stops being ambiguous and resolves to
`public.sessions`, while `daily_revenue` needs its qualifier again. Nothing reconnects, and
`pg.analytics.daily_revenue` keeps working throughout: hiding a schema bounds what a *bare* name
searches, never what a name written in full resolves to.

The keys line up with the file half: `user_id` is `users.parquet`'s 1-5, and `sku` is the variant
SKUs in `products.parquet` (`AL-WHT`, `TB-30L`, `CW-05`, `SH-PRO`, `OD-OAK`). Everything is dated
across January and February 2024, matching `events/year=2024/month=01|02`.

## Things to try

Three of these are saved queries in the project (**orders (unqualified)**, **files x database**,
**same name, two sources**).

```sql
-- Unqualified, and it reaches the server.
SELECT * FROM orders ORDER BY placed_at;

-- A file, a CSV and a live database in one join, none of them qualified.
SELECT r.region, u.name, count(*) AS orders, sum(o.total) AS spend
FROM orders o
JOIN users u ON u.user_id = o.user_id
JOIN regions r ON r.country = u.country
GROUP BY r.region, u.name
ORDER BY spend DESC;

-- A view, and the second schema.
SELECT * FROM open_tickets;
SELECT * FROM daily_revenue ORDER BY day;

-- The project's own table wins its name; the server's is one qualifier away.
SELECT count(*) FROM products;            -- 6, from products.parquet
SELECT count(*) FROM pg.public.products;  -- 5, from the server

-- Two relations of one name: refused, with both addresses in the message.
SELECT * FROM sessions;

-- Read-only in v1: a write names the connection rather than claiming the table is missing.
INSERT INTO orders VALUES (1);

-- Pushed down as PostgreSQL's own operator — check the plan view.
SELECT count(*) FROM orders WHERE (tags ->> 'channel') = 'web';
```

## Why trust authentication

The server takes any password because the committed `project.json` stores only the *expectation*
of one: a fixture that wanted a password would make every checkout open the connection editor and
type it before anything went green, and the password itself could never be committed. It is
published on `127.0.0.1` only, and it holds nothing but this fixture.

If you would rather exercise the keystore path, set `POSTGRES_PASSWORD` on the container, flip the
connection's PASSWORD row to *Keystore* in the editor and enter it there — the def gains no
machine-local id either way.
