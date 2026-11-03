from django.test.runner import DiscoverRunner

from documents.authz import close_engines


class Runner(DiscoverRunner):
    def teardown_databases(self, old_config, **kwargs):
        close_engines()
        super().teardown_databases(old_config, **kwargs)
