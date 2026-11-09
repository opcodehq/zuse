# Imported from Zuse desktop catalogs. See migration.json for provenance.
# Edit individual translations here; do not regenerate this file.

tools-code_block_copy_code = Copier le code

tools-tool_row_error = Erreur

tools-tool_row_no_matches = Aucun match.

tools-tool_row_empty_directory = Répertoire vide.

tools-tool_row_plan = Planifier

tools-tool_row_no_plan_body = (Pas de corps de plan.)

tools-tool_row_approved = Approuvé

tools-tool_row_cancelled = Annulé

tools-tool_row_sending_to_thread = Envoi vers le fil de discussion...

tools-tool_row_creating_session_tab = Création de l'onglet Session...

tools-tool_row_creating_chat = Création d'une conversation...

tools-tool_row_open_chat = Ouvrir le chat

tools-tool_row_chat_not_loaded_yet = Chat pas encore chargé

tools-tool_row_cancelled_2 = (annulé)

tools-tool_row_question = question

tools-tool_row_questions = des questions

tools-tool_row_copy_q_a = Copier les questions et réponses

tools-tool_row_answered = Répondu

tools-tool_row_user_input = Entrée de l'utilisateur

tools-tool_row_result = Résultat

tools-tool_row_error_2 = erreur

tools-tool_row_thought_content_was_redacted_by_the_model = Le contenu de la pensée a été rédigé par le modèle.

tools-tool_row_the_model_produced_a_thinking_block_the_sdk_forwarded_its_signed_recei = Le modèle a produit un bloc de réflexion (le SDK a transmis son reçu signé) mais le texte sous-jacent a été filtré par le SDK de l'agent Anthropic avant de nous parvenir. Nous ne pouvons pas exposer les véritables pensées sans contourner le SDK officiel.

tools-tool_row_thinking = Penser

tools-code_block_code = Code{ " " }{ $name }

tools-tool_row_open = Ouvrir{ " " }{ $shownPath }

tools-tool_row_background_task = Tâche en arrière-plan

tools-tool_row_viewed_image = Image consultée

tools-tool_row_generating_image = Génération de l’image

tools-tool_row_generated_image = Image générée

tools-tool_row_copy_image = Copier l’image

tools-tool_row_open_image = Ouvrir l’image

tools-tool_row_image_generation_failed = Échec de la génération de l’image

tools-tool_row_preview_of = Aperçu de{ " " }{ $value1 }

tools-tool_row_grep = Grep

tools-tool_row_glob = Glob

tools-tool_row_list = Liste

tools-tool_row_agent = Agent

tools-tool_row_webfetch = WebFetch

tools-tool_row_websearch = Recherche sur le Web

tools-tool_row_todowrite = TodoEcrire

tools-tool_row_spawn_agent = Générer des agents{ " " }{ $n }

tools-tool_row_browse = Parcourir

tools-tool_row_screenshot = Capture d'écran

tools-tool_row_read_page = Lire la page

tools-tool_row_click = Cliquez

tools-tool_row_type = Tapez

tools-tool_row_wait = Attends

tools-tool_row_scroll = Faire défiler

tools-tool_row_hover = Survolez

tools-tool_row_select = Sélectionnez

tools-tool_row_press = Presse

tools-tool_row_console = Console

tools-tool_row_log_in = Connectez-vous

tools-tool_row_max = { $value1 }{ " " }maximum ·{ " " }

tools-tool_row_failed = échoué

tools-tool_row_prompt = Invite

tools-tool_row_reply = Répondre

tools-tool_row_no_results_returned = (aucun résultat renvoyé)

tools-tool_row_threads = Sujets :

tools-tool_row_captured_the_visible_page = Capturé la page visible.

tools-tool_row_browser_screenshot = Capture d'écran du navigateur

tools-tool_row_pattern_sentence = modèle <part0>{ $pattern }</part0>

tools-tool_row_scope_sentence = portée <part0>{ $where }</part0>

tools-tool_row_model_sentence = Modèle : <part0>{ $model }</part0>

tools-tool_row_live_states_tracked_sentence = États en direct : { $value }{ " " }suivi

tools-tool_row_spawn_agent_one = Agent{ " " }{ $n }

tools-tool_row_spawn_agent_other = Générer des agents{ " " }{ $n }

tools-tool_row_edit_plural0_sentence = { $editCount }{ " " }modifications

tools-tool_row_edit_plural0_sentence_one = { $editCount }

tools-tool_row_edit_plural0_sentence_other = { $editCount }{ " " }modifications

tools-code_block_copy = Copier{ " " }{ $name }

tools-code_block_empty = (vide)

tools-tool_row_empty = (vide)

tools-tool_row_spawn_agent_many = Générer des agents{ " " }{ $n }

tools-tool_row_edit_plural0_sentence_many = { $editCount }{ " " }modifications

tools-tool_row_message_sent_to_thread = Message envoyé au fil

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_spawn_agent-plural =
    { $count ->
       [one] Agent{ " " }{ $n }
       *[other] Générer des agents{ " " }{ $n }
       [many] Générer des agents{ " " }{ $n }
    }

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_edit_plural0_sentence-plural =
    { $count ->
       [one] { $editCount }
       *[other] { $editCount }{ " " }modifications
       [many] { $editCount }{ " " }modifications
    }
