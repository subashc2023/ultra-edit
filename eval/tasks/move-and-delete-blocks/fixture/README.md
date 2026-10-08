# worker

Background job worker. Handlers live in `app/handlers.py`; the scheduler only
enqueues kinds listed in `app/registry.py`. See `docs/handlers.md` for the
handler reference.
