import uuid

from django.contrib.auth import get_user_model
from django.core.management.base import BaseCommand
from django.db import connection, transaction

import pemba

from documents.models import Document, Folder, Team
from documents.services import (
    add_team_member,
    can_view,
    create_document,
    share_folder_with_team,
    share_with_team,
    visible_documents,
)

User = get_user_model()


class PaymentFailed(Exception):
    pass


class Command(BaseCommand):
    help = "Walks through commit, rollback, checks and lookups on the configured database."

    def handle(self, *args, **options):
        suffix = uuid.uuid4().hex[:8]
        anne = User.objects.create(username=f"anne-{suffix}")
        bob = User.objects.create(username=f"bob-{suffix}")
        print("database:", connection.vendor)

        document, pending = create_document("Roadmap", anne)
        print("anne can view her document:", can_view(anne, document, at_least=pending))
        print("bob can view it:", can_view(bob, document, at_least=pending))

        try:
            with transaction.atomic():
                secret, lost = create_document("Secret", anne)
                raise PaymentFailed
        except PaymentFailed:
            pass
        print("rolled back, row exists:", Document.objects.filter(pk=secret.pk).exists())
        print("rolled back, anne can view:", can_view(anne, secret))
        try:
            print("rolled back, pending revision:", lost.revision())
        except pemba.WriteRolledBack as error:
            print("rolled back, pending revision:", error.code)

        team = Team.objects.create(name="Design")
        add_team_member(team, bob)
        folder = Folder.objects.create(name="Specs")
        in_folder, _ = create_document("In a shared folder", anne, folder)
        share_with_team(document, team)
        pending = share_folder_with_team(folder, team)
        titles = sorted(d.title for d in visible_documents(bob, at_least=pending))
        print("bob can now view:", titles)
