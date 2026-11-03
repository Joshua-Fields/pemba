from django.db import transaction

from .authz import get_engine, write_conn
from .models import Document


def create_document(title, owner, folder=None):
    engine = get_engine()
    with transaction.atomic():
        document = Document.objects.create(title=title, owner=owner, folder=folder)
        relationships = [f"document:{document.id}#owner@user:{owner.pk}"]
        if folder is not None:
            relationships.append(f"document:{document.id}#parent@folder:{folder.id}")
        pending = engine.write(touch=relationships, conn=write_conn())
    return document, pending


def delete_document(document):
    engine = get_engine()
    document_id = str(document.id)

    def remove_relationships():
        while engine.delete_relationships(resource_type="document", resource_id=document_id).more:
            pass

    with transaction.atomic():
        document.delete()
        # delete_relationships() can't join a transaction, so it runs after the commit.
        transaction.on_commit(remove_relationships)


def add_team_member(team, user):
    engine = get_engine()
    with transaction.atomic():
        team.members.add(user)
        return engine.write(touch=[f"team:{team.id}#member@user:{user.pk}"], conn=write_conn())


def share_with_team(document, team):
    engine = get_engine()
    with transaction.atomic():
        return engine.write(
            touch=[f"document:{document.id}#viewer@team:{team.id}#member"],
            conn=write_conn(),
        )


def share_folder_with_team(folder, team):
    engine = get_engine()
    with transaction.atomic():
        return engine.write(
            touch=[f"folder:{folder.id}#viewer@team:{team.id}#member"],
            conn=write_conn(),
        )


def can_view(user, document, at_least=None):
    return get_engine().check(
        f"document:{document.id}", "view", f"user:{user.pk}", at_least=at_least
    )


def visible_documents(user, at_least=None):
    ids = get_engine().iter_resources("document", "view", f"user:{user.pk}", at_least=at_least)
    return Document.objects.filter(pk__in=list(ids))
