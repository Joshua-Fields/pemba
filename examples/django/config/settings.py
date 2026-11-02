import os
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
SECRET_KEY = "example-only"
USE_TZ = True
DEFAULT_AUTO_FIELD = "django.db.models.BigAutoField"
INSTALLED_APPS = [
    "django.contrib.contenttypes",
    "django.contrib.auth",
    "documents",
]
TEST_RUNNER = "config.test_runner.Runner"

if os.environ.get("DJANGO_DB", "postgres") == "postgres":
    DATABASES = {
        "default": {
            "ENGINE": "django.db.backends.postgresql",
            "NAME": os.environ.get("PGDATABASE", "app"),
            "USER": os.environ.get("PGUSER", "app"),
            "PASSWORD": os.environ.get("PGPASSWORD", "devonly"),
            "HOST": os.environ.get("PGHOST", "localhost"),
            "PORT": os.environ.get("PGPORT", "5432"),
        }
    }
else:
    DATABASES = {
        "default": {
            "ENGINE": "django.db.backends.sqlite3",
            "NAME": BASE_DIR / "app.sqlite3",
            # Take the write lock at BEGIN (Django 5.1+), so a transaction that reads
            # first can't fail with "database is locked" when it later writes.
            "OPTIONS": {"transaction_mode": "IMMEDIATE"},
            # pemba needs a real file, not Django's default in-memory test database.
            "TEST": {"NAME": BASE_DIR / "test_app.sqlite3"},
        }
    }
