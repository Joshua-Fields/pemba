from django.contrib.auth import get_user_model
from django.db import connection, transaction
from django.test import TestCase, TransactionTestCase

from .authz import get_engine
from .models import Document, Folder, Team
from .services import (
    add_team_member,
    can_view,
    create_document,
    delete_document,
    share_folder_with_team,
    share_with_team,
    visible_documents,
)

User = get_user_model()


class PaymentFailed(Exception):
    pass


def stored_relationships(document):
    table = "authz.pemba_relationships" if connection.vendor == "postgresql" else "pemba_relationships"
    with connection.cursor() as cursor:
        cursor.execute(f"SELECT count(*) FROM {table} WHERE resource_id = %s", [str(document.id)])
        return cursor.fetchone()[0]


class DocumentPermissionTests(TransactionTestCase):
    def setUp(self):
        self.anne = User.objects.create(username="anne")
        self.bob = User.objects.create(username="bob")

    def test_owner_can_view_after_commit(self):
        document, pending = create_document("Roadmap", self.anne)

        self.assertTrue(can_view(self.anne, document, at_least=pending))
        self.assertFalse(can_view(self.bob, document, at_least=pending))

    def test_rollback_removes_the_row_and_the_permission(self):
        with self.assertRaises(PaymentFailed):
            with transaction.atomic():
                document, pending = create_document("Secret", self.anne)
                raise PaymentFailed

        self.assertFalse(Document.objects.filter(pk=document.pk).exists())
        self.assertEqual(stored_relationships(document), 0)
        self.assertFalse(can_view(self.anne, document))

    def test_team_and_folder_inheritance(self):
        team = Team.objects.create(name="Design")
        add_team_member(team, self.bob)

        folder = Folder.objects.create(name="Specs")
        in_folder, _ = create_document("In a shared folder", self.anne, folder)
        direct, _ = create_document("Shared directly", self.anne)
        private, _ = create_document("Private", self.anne)

        share_with_team(direct, team)
        pending = share_folder_with_team(folder, team)

        self.assertTrue(can_view(self.bob, in_folder, at_least=pending))
        self.assertTrue(can_view(self.bob, direct, at_least=pending))
        self.assertFalse(can_view(self.bob, private, at_least=pending))
        self.assertCountEqual(visible_documents(self.bob, at_least=pending), [in_folder, direct])

    def test_read_your_write_from_on_commit(self):
        seen = []
        with transaction.atomic():
            document, pending = create_document("Roadmap", self.anne)
            transaction.on_commit(
                lambda: seen.append(can_view(self.anne, document, at_least=pending))
            )

        self.assertEqual(seen, [True])

    def test_keep_the_revision_for_a_later_request(self):
        document, pending = create_document("Roadmap", self.anne)

        revision = int(pending.revision())  # after the commit, for example in the session

        self.assertTrue(can_view(self.anne, document, at_least=revision))

    def test_delete_removes_every_relationship(self):
        team = Team.objects.create(name="Design")
        add_team_member(team, self.bob)
        document, _ = create_document("Old notes", self.anne)
        document_id = str(document.id)
        share_with_team(document, team)

        delete_document(document)

        engine = get_engine()
        self.assertNotIn(document_id, engine.iter_resources("document", "view", f"user:{self.anne.pk}"))
        self.assertNotIn(document_id, engine.iter_resources("document", "view", f"user:{self.bob.pk}"))


class WhyNotTestCase(TestCase):
    def test_the_engine_cannot_see_a_write_that_never_commits(self):
        anne = User.objects.create(username="anne")

        # TestCase wraps this test in a transaction that always rolls back, and the
        # engine reads on its own connection, so it never sees this write.
        document, _ = create_document("Roadmap", anne)

        self.assertFalse(can_view(anne, document))
