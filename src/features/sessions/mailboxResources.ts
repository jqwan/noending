import { api } from "../../api";
import { createReadResource } from "../../hooks/readResource";

export const mailboxes = {
  all: createReadResource(id => api.mailboxList(id, "all")),
  unread: createReadResource(id => api.mailboxList(id, "unread")),
};
