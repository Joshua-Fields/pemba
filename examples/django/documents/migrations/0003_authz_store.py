from django.db import migrations

from documents.authz import migrate_store


def setup_store(apps, schema_editor):
    migrate_store()


class Migration(migrations.Migration):
    # pemba migrates on its own connection, so this can't sit inside a transaction.
    atomic = False
    dependencies = [("documents", "0002_authz_schema")]
    operations = [migrations.RunPython(setup_store, migrations.RunPython.noop)]
