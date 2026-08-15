-- The rows. Small, fixed and deterministic: a fixture you can assert against by eye.
--
-- Dated across January and February 2024 so a query lines up with the file half's
-- `events/year=2024/month=01|02` partitions.

INSERT INTO public.orders (order_id, user_id, sku, quantity, total, placed_at, tags) VALUES
  (5001, 1, 'AL-WHT', 1,  49.00, '2024-01-08 09:14:00+00', '{"channel":"web","gift":false}'),
  (5002, 1, 'SH-PRO', 1, 199.00, '2024-01-11 17:02:00+00', '{"channel":"web","gift":true}'),
  (5003, 2, 'TB-30L', 2, 240.00, '2024-01-14 08:45:00+00', '{"channel":"store"}'),
  (5004, 3, 'CW-05',  4,  98.00, '2024-01-19 12:30:00+00', '{"channel":"web","gift":false}'),
  (5005, 3, 'AL-WHT', 2,  98.00, '2024-01-22 19:55:00+00', '{"channel":"partner"}'),
  (5006, 4, 'OD-OAK', 1,  38.00, '2024-01-29 07:20:00+00', '{"channel":"web"}'),
  (5007, 5, 'TB-30L', 1, 120.00, '2024-02-02 15:11:00+00', '{"channel":"store","gift":true}'),
  (5008, 1, 'CW-05',  3,  73.50, '2024-02-06 10:05:00+00', '{"channel":"web"}'),
  (5009, 2, 'SH-PRO', 1, 199.00, '2024-02-09 21:40:00+00', '{"channel":"web","gift":false}'),
  (5010, 4, 'AL-WHT', 1,  49.00, '2024-02-13 13:27:00+00', '{"channel":"partner"}'),
  (5011, 4, 'SH-PRO', 2, 398.00, '2024-02-20 11:03:00+00', '{"channel":"web","gift":true}'),
  (5012, 5, 'OD-OAK', 3, 114.00, '2024-02-26 16:48:00+00', '{"channel":"store"}');

INSERT INTO public.shipments (shipment_id, order_id, carrier, shipped_at, delivered_at) VALUES
  (9001, 5001, 'DHL',    '2024-01-09 11:00:00+00', '2024-01-12 09:30:00+00'),
  (9002, 5002, 'DHL',    '2024-01-12 11:00:00+00', '2024-01-16 14:05:00+00'),
  (9003, 5003, 'Royal',  '2024-01-15 09:00:00+00', '2024-01-18 10:15:00+00'),
  (9004, 5004, 'SF',     '2024-01-20 08:00:00+00', NULL),
  (9005, 5005, 'SF',     '2024-01-23 08:00:00+00', '2024-01-27 17:45:00+00'),
  (9006, 5006, 'DPD',    '2024-01-30 10:00:00+00', '2024-02-01 12:00:00+00'),
  (9007, 5007, 'Royal',  '2024-02-03 09:30:00+00', '2024-02-07 08:20:00+00'),
  (9008, 5008, 'DHL',    '2024-02-07 12:00:00+00', NULL),
  (9009, 5009, 'Royal',  '2024-02-10 09:00:00+00', '2024-02-14 16:10:00+00'),
  (9010, 5010, 'DPD',    '2024-02-14 10:30:00+00', '2024-02-17 09:05:00+00'),
  (9011, 5011, 'DPD',    '2024-02-21 09:00:00+00', NULL),
  (9012, 5012, 'SF',     '2024-02-27 08:15:00+00', '2024-03-02 11:40:00+00');

INSERT INTO public.support_tickets
    (ticket_id, user_id, subject, status, priority, opened_at, closed_at) VALUES
  (301, 1, 'Lamp arrived without the shade',   'closed', 'normal', '2024-01-13 10:00:00+00', '2024-01-15 09:00:00+00'),
  (302, 3, 'Bottle lid leaks',                 'open',   'high',   '2024-01-21 14:20:00+00', NULL),
  (303, 3, 'Where is my order',                'open',   'normal', '2024-01-25 08:05:00+00', NULL),
  (304, 4, 'Clock runs two minutes fast',      'closed', 'low',    '2024-02-01 16:40:00+00', '2024-02-03 11:20:00+00'),
  (305, 5, 'Backpack strap stitching',         'open',   'high',   '2024-02-05 09:10:00+00', NULL),
  (306, 2, 'Headphones pair but do not charge','open',   'urgent', '2024-02-11 19:25:00+00', NULL),
  (307, 4, 'Invoice address wrong',            'closed', 'low',    '2024-02-22 07:55:00+00', '2024-02-22 15:30:00+00');

INSERT INTO public.products (sku, supplier, unit_cost, lead_time_days) VALUES
  ('AL-WHT', 'Northlight Works',  22.40,  9),
  ('TB-30L', 'Ridgeline Supply',  61.00, 21),
  ('CW-05',  'Northlight Works',   9.75,  6),
  ('SH-PRO', 'Ampere Audio',      88.50, 34),
  ('OD-OAK', 'Ridgeline Supply',  17.20, 14);

INSERT INTO public.sessions (session_id, user_id, started_at, minutes) VALUES
  (7001, 1, '2024-01-08 09:02:00+00', 18),
  (7002, 2, '2024-01-14 08:31:00+00',  7),
  (7003, 3, '2024-01-19 12:12:00+00', 26),
  (7004, 3, '2024-01-22 19:40:00+00',  4),
  (7005, 4, '2024-01-29 07:05:00+00', 11),
  (7006, 5, '2024-02-02 14:58:00+00', 33),
  (7007, 1, '2024-02-06 09:47:00+00',  9),
  (7008, 4, '2024-02-20 10:44:00+00', 21);

INSERT INTO analytics.daily_revenue (day, orders, revenue) VALUES
  ('2024-01-08', 1,  49.00),
  ('2024-01-11', 1, 199.00),
  ('2024-01-14', 1, 240.00),
  ('2024-01-19', 1,  98.00),
  ('2024-01-22', 1,  98.00),
  ('2024-01-29', 1,  38.00),
  ('2024-02-02', 1, 120.00),
  ('2024-02-06', 1,  73.50),
  ('2024-02-09', 1, 199.00),
  ('2024-02-13', 1,  49.00),
  ('2024-02-20', 1, 398.00),
  ('2024-02-26', 1, 114.00);

INSERT INTO analytics.sessions (day, user_id, sessions, minutes) VALUES
  ('2024-01-08', 1, 1, 18),
  ('2024-01-14', 2, 1,  7),
  ('2024-01-19', 3, 1, 26),
  ('2024-01-22', 3, 1,  4),
  ('2024-01-29', 4, 1, 11),
  ('2024-02-02', 5, 1, 33),
  ('2024-02-06', 1, 1,  9),
  ('2024-02-20', 4, 1, 21);

-- A view and a materialized view, so the tree's Tables / Views split has both to show and a bare
-- name resolves to something that is not a table. `pg_class`, not `pg_tables`, is why they are
-- listed at all.
CREATE VIEW public.open_tickets AS
SELECT ticket_id, user_id, subject, priority, opened_at
FROM public.support_tickets
WHERE status = 'open';

CREATE MATERIALIZED VIEW public.revenue_by_sku AS
SELECT o.sku,
       count(*)      AS orders,
       sum(o.total)  AS revenue,
       sum(o.quantity * p.unit_cost) AS cost
FROM public.orders o
JOIN public.products p USING (sku)
GROUP BY o.sku;
