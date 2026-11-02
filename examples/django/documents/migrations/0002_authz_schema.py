from django.db import migrations


def create_schema(apps, schema_editor):
    if schema_editor.connection.vendor == "postgresql":
        schema_editor.execute("CREATE SCHEMA IF NOT EXISTS authz")


class Migration(migrations.Migration):
    dependencies = [("documents", "0001_initial")]
    operations = [migrations.RunPython(create_schema, migrations.RunPython.noop)]
