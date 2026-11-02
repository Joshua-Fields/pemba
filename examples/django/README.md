# pemba with Django

A small Django project whose per-object permissions are written in the same database
transaction as its rows: when `transaction.atomic()` rolls back, the permission goes too.
It runs on Postgres and on SQLite.

- `documents/authz.py`: the schema, and opening the engine from Django's settings.
- `documents/services.py`: creating, sharing, deleting, checking and listing documents.
- `documents/tests.py`: rollback, inheritance, read-after-write, and why not `TestCase`.
- `documents/migrations/`: creating pemba's schema and tables from Django migrations.
- `config/test_runner.py`: releases the engine so Django can drop the test database.

## Run it

Python 3.12 and Django 5.1 or newer (SQLite's `transaction_mode` option needs 5.1).

```sh
python -m venv .venv
# POSIX: . .venv/bin/activate
# PowerShell: .venv\Scripts\Activate.ps1
python -m pip install -r requirements.txt
```

On Postgres, with a disposable server:

```sh
docker run -d --name pemba-django -e POSTGRES_USER=app -e POSTGRES_PASSWORD=devonly \
  -e POSTGRES_DB=app -p 5432:5432 postgres:18
python manage.py migrate
python manage.py demo
python manage.py test
```

The settings read `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD` and `PGDATABASE`.

On SQLite, set `DJANGO_DB=sqlite` and run the last three commands.

See [transactions](../../docs/guides/transactions.md) and
[consistency](../../docs/guides/consistency.md) for the rules behind this.
