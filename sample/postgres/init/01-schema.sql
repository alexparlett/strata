-- The sample database's shape. Every relation here is chosen to be about something the sample
-- project's *files* cannot answer on their own, so a query has a reason to cross the boundary.
--
-- Keys into the file half: `user_id` matches `users.parquet` (1-5), `sku` matches the variant
-- SKUs in `products.parquet` (AL-WHT, TB-30L, CW-05, SH-PRO, OD-OAK).

CREATE SCHEMA analytics;

-- ---------------------------------------------------------------------------------------------
-- public — what the connection shows in the data-sources tree
-- ---------------------------------------------------------------------------------------------

-- The headline: a name nothing in the project has, so `SELECT * FROM orders` resolves here.
-- `tags` is jsonb, which is what the JSON accessors push down to the server (`tags ->> 'channel'`).
CREATE TABLE public.orders (
    order_id   int PRIMARY KEY,
    user_id    int          NOT NULL,
    sku        text         NOT NULL,
    quantity   int          NOT NULL,
    total      numeric(9,2) NOT NULL,
    placed_at  timestamptz  NOT NULL,
    tags       jsonb
);

CREATE TABLE public.shipments (
    shipment_id  int PRIMARY KEY,
    order_id     int  NOT NULL REFERENCES public.orders (order_id),
    carrier      text NOT NULL,
    shipped_at   timestamptz,
    delivered_at timestamptz
);

CREATE TABLE public.support_tickets (
    ticket_id int PRIMARY KEY,
    user_id   int  NOT NULL,
    subject   text NOT NULL,
    status    text NOT NULL,
    priority  text NOT NULL,
    opened_at timestamptz NOT NULL,
    closed_at timestamptz
);

-- **Deliberately the same name as the project's `products.parquet`.** A bare `products` is the
-- workspace's, always — this one is reached as `pg.public.products`. It holds what a file cannot:
-- supply-side facts that change per day.
CREATE TABLE public.products (
    sku            text PRIMARY KEY,
    supplier       text         NOT NULL,
    unit_cost      numeric(9,2) NOT NULL,
    lead_time_days int          NOT NULL
);

-- **Deliberately the same name as `analytics.sessions`.** A bare `sessions` is ambiguous and is
-- refused by name — see the README.
CREATE TABLE public.sessions (
    session_id int PRIMARY KEY,
    user_id    int NOT NULL,
    started_at timestamptz NOT NULL,
    minutes    int NOT NULL
);

-- ---------------------------------------------------------------------------------------------
-- analytics — a schema the connection does NOT display, and still resolves
-- ---------------------------------------------------------------------------------------------

CREATE TABLE analytics.daily_revenue (
    day     date PRIMARY KEY,
    orders  int          NOT NULL,
    revenue numeric(9,2) NOT NULL
);

CREATE TABLE analytics.sessions (
    day      date NOT NULL,
    user_id  int  NOT NULL,
    sessions int  NOT NULL,
    minutes  int  NOT NULL,
    PRIMARY KEY (day, user_id)
);
